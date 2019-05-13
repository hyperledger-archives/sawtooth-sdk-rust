/*
 * Copyright 2017 Bitwise IO, Inc.
 *
 * Licensed under the Apache License, Version 2.0 (the "License");
 * you may not use this file except in compliance with the License.
 * You may obtain a copy of the License at
 *
 *     http://www.apache.org/licenses/LICENSE-2.0
 *
 * Unless required by applicable law or agreed to in writing, software
 * distributed under the License is distributed on an "AS IS" BASIS,
 * WITHOUT WARRANTIES OR CONDITIONS OF ANY KIND, either express or implied.
 * See the License for the specific language governing permissions and
 * limitations under the License.
 * -----------------------------------------------------------------------------
 */

#![allow(unknown_lints)]

extern crate ctrlc;
extern crate protobuf;
extern crate rand;
extern crate zmq;

use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc::RecvTimeoutError;
use std::sync::Arc;
use std::time::Duration;

use self::rand::Rng;

pub mod handler;
mod zmq_context;

use crate::messages::network::PingResponse;
use crate::messages::processor::TpProcessRequest;
use crate::messages::processor::TpProcessResponse;
use crate::messages::processor::TpProcessResponse_Status;
use crate::messages::processor::TpRegisterRequest;
use crate::messages::processor::TpRegisterRequest_TpProcessRequestHeaderStyle;
use crate::messages::processor::TpRegisterResponse;
use crate::messages::processor::TpRegisterResponse_Status;
use crate::messages::processor::TpUnregisterRequest;
use crate::messages::validator::Message_MessageType;
use crate::messaging::stream::MessageConnection;
use crate::messaging::stream::MessageSender;
use crate::messaging::stream::ReceiveError;
use crate::messaging::stream::SendError;
use crate::messaging::zmq_stream::ZmqMessageConnection;
use crate::messaging::zmq_stream::ZmqMessageSender;
use protobuf::Message as M;
use protobuf::RepeatedField;

use self::handler::ApplyError;
use self::handler::TransactionHandler;
use self::zmq_context::ZmqTransactionContext;

// This is the version used by SDK to match if validator supports feature
// it requested during registration. It should only be incremented when
// there are changes in TpRegisterRequest. Remember to sync this
// information in validator if changed.
// Note: SDK_PROTOCOL_VERSION is the highest version the SDK supports
#[derive(Debug, Clone)]
enum FeatureVersion {
    FeatureUnused = 0,
    FeatureCustomHeaderStyle = 1,
}

impl FeatureVersion {
    pub const FEATURE_UNUSED: FeatureVersion = FeatureVersion::FeatureUnused;
    pub const FEATURE_CUSTOM_HEADER_STYLE: FeatureVersion =
        FeatureVersion::FeatureCustomHeaderStyle;
    pub const SDK_PROTOCOL_VERSION: FeatureVersion = FeatureVersion::FeatureCustomHeaderStyle;
}

/// Generates a random correlation id for use in Message
fn generate_correlation_id() -> String {
    const LENGTH: usize = 16;
    rand::thread_rng().gen_ascii_chars().take(LENGTH).collect()
}

pub struct TransactionProcessor<'a> {
    endpoint: String,
    conn: ZmqMessageConnection,
    handlers: Vec<&'a dyn TransactionHandler>,
    highest_sdk_feature_requested: FeatureVersion,
    header_style: TpRegisterRequest_TpProcessRequestHeaderStyle,
}

impl<'a> TransactionProcessor<'a> {
    /// TransactionProcessor is for communicating with a
    /// validator and routing transaction processing requests to a registered
    /// handler. It uses ZMQ and channels to handle requests concurrently.
    pub fn new(endpoint: &str) -> TransactionProcessor {
        TransactionProcessor {
            endpoint: String::from(endpoint),
            conn: ZmqMessageConnection::new(endpoint),
            handlers: Vec::new(),
            highest_sdk_feature_requested: FeatureVersion::FEATURE_UNUSED,
            header_style: TpRegisterRequest_TpProcessRequestHeaderStyle::HEADER_STYLE_UNSET,
        }
    }

    /// Adds a transaction family handler
    ///
    /// # Arguments
    ///
    /// * handler - the handler to be added
    pub fn add_handler(&mut self, handler: &'a dyn TransactionHandler) {
        self.handlers.push(handler);
    }

    /// Set header style flag, this is used when validator sends TpProcessRequest
    /// to either send raw header bytes or deserialized transaction header.
    ///
    /// # Arguments
    ///
    /// * style - header style required in TpProcessRequest
    pub fn set_header_style(&mut self, style: TpRegisterRequest_TpProcessRequestHeaderStyle) {
        if FeatureVersion::FeatureCustomHeaderStyle as u32
            > self.highest_sdk_feature_requested.clone() as u32
        {
            self.highest_sdk_feature_requested = FeatureVersion::FeatureCustomHeaderStyle;
        }
        self.header_style = style;
    }

    fn register(&mut self, sender: &ZmqMessageSender, unregister: &Arc<AtomicBool>) -> bool {
        for handler in &self.handlers {
            for version in handler.family_versions() {
                let mut request = TpRegisterRequest::new();
                request.set_family(handler.family_name().clone());
                request.set_version(version.clone());
                request.set_namespaces(RepeatedField::from_vec(handler.namespaces().clone()));
                request.set_protocol_version(self.highest_sdk_feature_requested.clone() as u32);
                request.set_request_header_style(self.header_style.clone());
                info!(
                    "sending TpRegisterRequest: {} {}",
                    &handler.family_name(),
                    &version
                );
                let serialized = match request.write_to_bytes() {
                    Ok(serialized) => serialized,
                    Err(err) => {
                        error!("Serialization failed: {}", err);
                        // try reconnect
                        return false;
                    }
                };
                let x: &[u8] = &serialized;

                let mut future = match sender.send(
                    Message_MessageType::TP_REGISTER_REQUEST,
                    &generate_correlation_id(),
                    x,
                ) {
                    Ok(fut) => fut,
                    Err(err) => {
                        error!("Registration failed: {}", err);
                        // try reconnect
                        return false;
                    }
                };

                // Absorb the TpRegisterResponse message
                loop {
                    match future.get_timeout(Duration::from_millis(10000)) {
                        Ok(response) => {
                            let resp: TpRegisterResponse =
                                match protobuf::parse_from_bytes(&response.get_content()) {
                                    Ok(read_response) => read_response,
                                    Err(_) => {
                                        unregister.store(true, Ordering::SeqCst);
                                        error!("Error while unpacking TpRegisterResponse");
                                        return false;
                                    }
                                };
                            // Validator gives backward compatible support, do not proceed if SDK
                            // is expecting a feature which validator cannot provide
                            if resp.get_protocol_version()
                                != self.highest_sdk_feature_requested.clone() as u32
                            {
                                unregister.store(true, Ordering::SeqCst);
                                error!(
                                    "Validator version {} does not support \
                                    requested feature by SDK version {:?}. \
                                    Unregistering with the validator.",
                                    resp.get_protocol_version(),
                                    self.highest_sdk_feature_requested
                                );
                                return false;
                            }
                            if resp.get_status() == TpRegisterResponse_Status::ERROR {
                                unregister.store(true, Ordering::SeqCst);
                                error!("Transaction processor registration failed");
                                return false;
                            }
                            break;
                        }
                        Err(_) => {
                            if unregister.load(Ordering::SeqCst) {
                                return false;
                            }
                        }
                    };
                }
            }
        }
        true
    }

    fn unregister(&mut self, sender: &ZmqMessageSender) {
        let request = TpUnregisterRequest::new();
        info!("sending TpUnregisterRequest");
        let serialized = match request.write_to_bytes() {
            Ok(serialized) => serialized,
            Err(err) => {
                error!("Serialization failed: {}", err);
                return;
            }
        };
        let x: &[u8] = &serialized;

        let mut future = match sender.send(
            Message_MessageType::TP_UNREGISTER_REQUEST,
            &generate_correlation_id(),
            x,
        ) {
            Ok(fut) => fut,
            Err(err) => {
                error!("Unregistration failed: {}", err);
                return;
            }
        };
        // Absorb the TpUnregisterResponse message, wait one second for response then continue
        match future.get_timeout(Duration::from_millis(1000)) {
            Ok(_) => (),
            Err(err) => {
                info!("Unregistration failed: {}", err);
            }
        };
    }

    /// Connects the transaction processor to a validator and starts
    /// listening for requests and routing them to an appropriate
    /// transaction handler.
    #[allow(clippy::cognitive_complexity)]
    pub fn start(&mut self) {
        let unregister = Arc::new(AtomicBool::new(false));
        let r = unregister.clone();
        ctrlc::set_handler(move || {
            r.store(true, Ordering::SeqCst);
        })
        .expect("Error setting Ctrl-C handler");

        let mut first_time = true;
        let mut restart = true;

        while restart {
            info!("connecting to endpoint: {}", self.endpoint);
            if first_time {
                first_time = false;
            } else {
                self.conn = ZmqMessageConnection::new(&self.endpoint);
            }
            let (mut sender, receiver) = self.conn.create();

            if unregister.load(Ordering::SeqCst) {
                self.unregister(&sender);
                restart = false;
                continue;
            }

            // if registration is not succesful, retry
            if !self.register(&sender, &unregister.clone()) {
                continue;
            }

            loop {
                if unregister.load(Ordering::SeqCst) {
                    self.unregister(&sender);
                    restart = false;
                    break;
                }
                match receiver.recv_timeout(Duration::from_millis(1000)) {
                    Ok(r) => {
                        // Check if we have a message
                        let message = match r {
                            Ok(message) => message,
                            Err(ReceiveError::DisconnectedError) => {
                                info!("Trying to Reconnect");
                                break;
                            }
                            Err(err) => {
                                error!("Error: {}", err);
                                continue;
                            }
                        };

                        trace!("Message: {}", message.get_correlation_id());

                        match message.get_message_type() {
                            Message_MessageType::TP_PROCESS_REQUEST => {
                                let request: TpProcessRequest =
                                    match protobuf::parse_from_bytes(&message.get_content()) {
                                        Ok(request) => request,
                                        Err(err) => {
                                            error!("Cannot parse TpProcessRequest: {}", err);
                                            continue;
                                        }
                                    };

                                let mut context = ZmqTransactionContext::new(
                                    request.get_context_id(),
                                    sender.clone(),
                                );

                                let mut response = TpProcessResponse::new();
                                match self.handlers[0].apply(&request, &mut context) {
                                    Ok(()) => {
                                        info!("TP_PROCESS_REQUEST sending TpProcessResponse: OK");
                                        response.set_status(TpProcessResponse_Status::OK);
                                    }
                                    Err(ApplyError::InvalidTransaction(msg)) => {
                                        info!(
                                            "TP_PROCESS_REQUEST sending TpProcessResponse: {}",
                                            &msg
                                        );
                                        response.set_status(
                                            TpProcessResponse_Status::INVALID_TRANSACTION,
                                        );
                                        response.set_message(msg);
                                    }
                                    Err(err) => {
                                        info!(
                                            "TP_PROCESS_REQUEST sending TpProcessResponse: {}",
                                            err
                                        );
                                        response
                                            .set_status(TpProcessResponse_Status::INTERNAL_ERROR);
                                        response.set_message(err.to_string());
                                    }
                                };

                                let serialized = match response.write_to_bytes() {
                                    Ok(serialized) => serialized,
                                    Err(err) => {
                                        error!("Serialization failed: {}", err);
                                        continue;
                                    }
                                };

                                match sender.reply(
                                    Message_MessageType::TP_PROCESS_RESPONSE,
                                    message.get_correlation_id(),
                                    &serialized,
                                ) {
                                    Ok(_) => (),
                                    Err(SendError::DisconnectedError) => {
                                        error!("DisconnectedError");
                                        break;
                                    }
                                    Err(SendError::TimeoutError) => error!("TimeoutError"),
                                    Err(SendError::UnknownError) => {
                                        restart = false;
                                        println!("UnknownError");
                                        break;
                                    }
                                };
                            }
                            Message_MessageType::PING_REQUEST => {
                                trace!("sending PingResponse");
                                let response = PingResponse::new();
                                let serialized = match response.write_to_bytes() {
                                    Ok(serialized) => serialized,
                                    Err(err) => {
                                        error!("Serialization failed: {}", err);
                                        continue;
                                    }
                                };
                                match sender.reply(
                                    Message_MessageType::PING_RESPONSE,
                                    message.get_correlation_id(),
                                    &serialized,
                                ) {
                                    Ok(_) => (),
                                    Err(SendError::DisconnectedError) => {
                                        error!("DisconnectedError");
                                        break;
                                    }
                                    Err(SendError::TimeoutError) => error!("TimeoutError"),
                                    Err(SendError::UnknownError) => {
                                        restart = false;
                                        println!("UnknownError");
                                        break;
                                    }
                                };
                            }
                            _ => {
                                info!(
                                    "Transaction Processor recieved invalid message type: {:?}",
                                    message.get_message_type()
                                );
                            }
                        }
                    }
                    Err(RecvTimeoutError::Timeout) => (),
                    Err(err) => {
                        error!("Error: {}", err);
                    }
                }
            }
            sender.close();
        }
    }
}
