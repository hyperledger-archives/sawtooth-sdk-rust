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
use crate::messages::processor::TpUnregisterRequest;
use crate::messages::validator::Message_MessageType;
use crate::messaging::stream::MessageSender;
use crate::messaging::stream::ReceiveError;
use crate::messaging::stream::SendError;
use crate::messaging::stream::{MessageConnection, MessageReceiver};
use crate::messaging::zmq_stream::ZmqMessageConnection;
use crate::messaging::zmq_stream::ZmqMessageSender;
use protobuf::Message as M;
use protobuf::RepeatedField;
use rand::distributions::Alphanumeric;

use self::handler::TransactionHandler;
use self::handler::{ApplyError, TransactionContext};
use self::zmq_context::ZmqTransactionContext;

/// Generates a random correlation id for use in Message
fn generate_correlation_id() -> String {
    const LENGTH: usize = 16;
    rand::thread_rng()
        .sample_iter(Alphanumeric)
        .take(LENGTH)
        .map(char::from)
        .collect()
}

pub struct EmptyTransactionContext {
    inner: Arc<InnerEmptyContext>,
}

impl Clone for EmptyTransactionContext {
    fn clone(&self) -> Self {
        Self {
            inner: Arc::clone(&self.inner),
        }
    }
}

struct InnerEmptyContext {
    context: Box<dyn TransactionContext + Send + Sync>,
    _sender: ZmqMessageSender,
    _receiver: std::sync::Mutex<MessageReceiver>,
}

impl EmptyTransactionContext {
    fn new(conn: &ZmqMessageConnection, timeout: Option<Duration>) -> Self {
        let (_sender, _receiver) = conn.create();
        Self {
            inner: Arc::new(InnerEmptyContext {
                context: Box::new(ZmqTransactionContext::with_timeout(
                    "",
                    _sender.clone(),
                    timeout,
                )),
                _receiver: std::sync::Mutex::new(_receiver),
                _sender,
            }),
        }
    }
    pub fn flush(&self) {
        if let Ok(rx) = self.inner._receiver.try_lock() {
            if let Ok(Ok(msg)) = rx.recv_timeout(Duration::from_millis(100)) {
                log::info!("Empty context received message : {:?}", msg);
            }
        }
    }
}

impl TransactionContext for EmptyTransactionContext {
    fn get_state_entries(
        &self,
        _addresses: &[String],
    ) -> Result<Vec<(String, Vec<u8>)>, handler::ContextError> {
        panic!("unsupported for an empty context")
    }

    fn set_state_entries(
        &self,
        _entries: Vec<(String, Vec<u8>)>,
    ) -> Result<(), handler::ContextError> {
        panic!("unsupported for an empty context")
    }

    fn delete_state_entries(
        &self,
        _addresses: &[String],
    ) -> Result<Vec<String>, handler::ContextError> {
        panic!("unsupported for an empty context")
    }

    fn add_receipt_data(&self, _data: &[u8]) -> Result<(), handler::ContextError> {
        panic!("unsupported for an empty context")
    }

    fn add_event(
        &self,
        _event_type: String,
        _attributes: Vec<(String, String)>,
        _data: &[u8],
    ) -> Result<(), handler::ContextError> {
        panic!("unsupported for an empty context")
    }

    fn get_sig_by_num(&self, block_num: u64) -> Result<String, handler::ContextError> {
        self.inner.context.get_sig_by_num(block_num)
    }

    fn get_reward_block_signatures(
        &self,
        block_id: &str,
        first_pred: u64,
        last_pred: u64,
    ) -> Result<Vec<String>, handler::ContextError> {
        self.inner
            .context
            .get_reward_block_signatures(block_id, first_pred, last_pred)
    }

    fn get_state_entries_by_prefix(
        &self,
        tip_id: &str,
        address: &str,
    ) -> Result<Vec<(String, Vec<u8>)>, handler::ContextError> {
        self.inner.context.get_state_entries_by_prefix(tip_id, address)
    }
}

pub struct TransactionProcessor<'a> {
    endpoint: String,
    conn: ZmqMessageConnection,
    handlers: Vec<&'a dyn TransactionHandler>,
    empty_contexts: Vec<EmptyTransactionContext>,
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
            empty_contexts: Vec::new(),
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

    pub fn empty_context(&mut self, timeout: Option<Duration>) -> EmptyTransactionContext {
        let context = EmptyTransactionContext::new(&self.conn, timeout);
        let context_cp = context.clone();
        self.empty_contexts.push(context);
        context_cp
    }

    fn register(&mut self, sender: &ZmqMessageSender, unregister: &Arc<AtomicBool>) -> bool {
        for handler in &self.handlers {
            for version in handler.family_versions() {
                let mut request = TpRegisterRequest::new();
                request.set_family(handler.family_name().clone());
                request.set_version(version.clone());
                request.set_namespaces(RepeatedField::from_vec(handler.namespaces().clone()));
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
                        Ok(_) => break,
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
                                let request = match TpProcessRequest::parse_from_bytes(
                                    &message.get_content(),
                                ) {
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
                                    Err(SendError::UnknownError(e)) => {
                                        restart = false;
                                        error!("UnknownError: {}", e);
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
                                    Err(SendError::UnknownError(e)) => {
                                        restart = false;
                                        error!("UnknownError: {}", e);
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
