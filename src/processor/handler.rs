/*
 * Copyright 2017 Bitwise IO, Inc.
 * Copyright 2019 Cargill Incorporated
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

extern crate protobuf;
extern crate rand;
extern crate zmq;

use std::collections::HashMap;
use std::error::Error as StdError;

use crate::messages::processor::TpProcessRequest;
use crate::messaging::stream::ReceiveError;
use crate::messaging::stream::SendError;

#[derive(Debug)]
pub enum ApplyError {
    /// Returned for an Invalid Transaction.
    InvalidTransaction(String),
    /// Returned when an internal error occurs during transaction processing.
    InternalError(String),
}

impl std::error::Error for ApplyError {}

impl std::fmt::Display for ApplyError {
    fn fmt(&self, f: &mut std::fmt::Formatter) -> std::fmt::Result {
        match *self {
            ApplyError::InvalidTransaction(ref s) => write!(f, "InvalidTransaction: {}", s),
            ApplyError::InternalError(ref s) => write!(f, "InternalError: {}", s),
        }
    }
}

#[derive(Debug)]
pub enum ContextError {
    /// Returned for an authorization error
    AuthorizationError(String),
    /// Returned when a error occurs due to missing info in a response
    ResponseAttributeError(String),
    /// Returned when there is an issues setting receipt data or events.
    TransactionReceiptError(String),
    /// Returned when a ProtobufError is returned during serializing
    SerializationError(Box<dyn StdError + Send + Sync + 'static>),
    /// Returned when an error is returned when sending a message
    SendError(Box<dyn StdError + Send + Sync + 'static>),
    /// Returned when an error is returned when sending a message
    ReceiveError(Box<dyn StdError + Send + Sync + 'static>),
}

impl std::error::Error for ContextError {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        match self {
            ContextError::SerializationError(err) => Some(&**err),
            ContextError::SendError(err) => Some(&**err),
            ContextError::ReceiveError(err) => Some(&**err),
            _ => None,
        }
    }
}

impl std::fmt::Display for ContextError {
    fn fmt(&self, f: &mut std::fmt::Formatter) -> std::fmt::Result {
        match *self {
            ContextError::AuthorizationError(ref s) => write!(f, "AuthorizationError: {}", s),
            ContextError::ResponseAttributeError(ref s) => {
                write!(f, "ResponseAttributeError: {}", s)
            }
            ContextError::TransactionReceiptError(ref s) => {
                write!(f, "TransactionReceiptError: {}", s)
            }
            ContextError::SerializationError(ref err) => write!(f, "SerializationError: {}", err),
            ContextError::SendError(ref err) => write!(f, "SendError: {}", err),
            ContextError::ReceiveError(ref err) => write!(f, "ReceiveError: {}", err),
        }
    }
}

impl From<ContextError> for ApplyError {
    fn from(context_error: ContextError) -> Self {
        match context_error {
            ContextError::TransactionReceiptError(..) => {
                ApplyError::InternalError(format!("{}", context_error))
            }
            _ => ApplyError::InvalidTransaction(format!("{}", context_error)),
        }
    }
}

impl From<protobuf::ProtobufError> for ContextError {
    fn from(e: protobuf::ProtobufError) -> Self {
        ContextError::SerializationError(Box::new(e))
    }
}

impl From<SendError> for ContextError {
    fn from(e: SendError) -> Self {
        ContextError::SendError(Box::new(e))
    }
}

impl From<ReceiveError> for ContextError {
    fn from(e: ReceiveError) -> Self {
        ContextError::ReceiveError(Box::new(e))
    }
}

pub trait TransactionContext {
    #[deprecated(
        since = "0.3.0",
        note = "please use `get_state_entry` or `get_state_entries` instead"
    )]
    /// get_state queries the validator state for data at each of the
    /// addresses in the given list. The addresses that have been set
    /// are returned. get_state is deprecated, please use get_state_entry or get_state_entries
    /// instead
    ///
    /// # Arguments
    ///
    /// * `addresses` - the addresses to fetch
    fn get_state(&self, addresses: &[String]) -> Result<Vec<(String, Vec<u8>)>, ContextError> {
        self.get_state_entries(addresses)
    }

    /// get_state_entry queries the validator state for data at the
    /// address given. If the  address is set, the data is returned.
    ///
    /// # Arguments
    ///
    /// * `address` - the address to fetch
    fn get_state_entry(&self, address: &str) -> Result<Option<Vec<u8>>, ContextError> {
        Ok(self
            .get_state_entries(&[address.to_string()])?
            .into_iter()
            .map(|(_, val)| val)
            .next())
    }

    /// get_state_entries queries the validator state for data at each of the
    /// addresses in the given list. The addresses that have been set
    /// are returned.
    ///
    /// # Arguments
    ///
    /// * `addresses` - the addresses to fetch
    fn get_state_entries(
        &self,
        addresses: &[String],
    ) -> Result<Vec<(String, Vec<u8>)>, ContextError>;

    #[deprecated(
        since = "0.3.0",
        note = "please use `set_state_entry` or `set_state_entries` instead"
    )]
    /// set_state requests that each address in the provided map be
    /// set in validator state to its corresponding value. set_state is deprecated, please use
    /// set_state_entry to set_state_entries instead
    ///
    /// # Arguments
    ///
    /// * `entries` - entries are a hashmap where the key is an address and value is the data
    fn set_state(&self, entries: HashMap<String, Vec<u8>>) -> Result<(), ContextError> {
        let state_entries: Vec<(String, Vec<u8>)> = entries.into_iter().collect();
        self.set_state_entries(state_entries)
    }

    /// set_state_entry requests that the provided address is set in the validator state to its
    /// corresponding value.
    ///
    /// # Arguments
    ///
    /// * `address` - address of where to store the data
    /// * `data` - payload is the data to store at the address
    fn set_state_entry(&self, address: String, data: Vec<u8>) -> Result<(), ContextError> {
        self.set_state_entries(vec![(address, data)])
    }

    /// set_state_entries requests that each address in the provided map be
    /// set in validator state to its corresponding value.
    ///
    /// # Arguments
    ///
    /// * `entries` - entries are a hashmap where the key is an address and value is the data
    fn set_state_entries(&self, entries: Vec<(String, Vec<u8>)>) -> Result<(), ContextError>;

    /// delete_state requests that each of the provided addresses be unset
    /// in validator state. A list of successfully deleted addresses is returned.
    /// delete_state is deprecated, please use delete_state_entry to delete_state_entries instead
    ///
    /// # Arguments
    ///
    /// * `addresses` - the addresses to delete
    #[deprecated(
        since = "0.3.0",
        note = "please use `delete_state_entry` or `delete_state_entries` instead"
    )]
    fn delete_state(&self, addresses: &[String]) -> Result<Vec<String>, ContextError> {
        self.delete_state_entries(addresses)
    }

    /// delete_state_entry requests that the provided address be unset
    /// in validator state. A list of successfully deleted addresses
    /// is returned.
    ///
    /// # Arguments
    ///
    /// * `address` - the address to delete
    fn delete_state_entry(&self, address: &str) -> Result<Option<String>, ContextError> {
        Ok(self
            .delete_state_entries(&[address.to_string()])?
            .into_iter()
            .next())
    }

    /// delete_state_entries requests that each of the provided addresses be unset
    /// in validator state. A list of successfully deleted addresses
    /// is returned.
    ///
    /// # Arguments
    ///
    /// * `addresses` - the addresses to delete
    fn delete_state_entries(&self, addresses: &[String]) -> Result<Vec<String>, ContextError>;

    /// add_receipt_data adds a blob to the execution result for this transaction
    ///
    /// # Arguments
    ///
    /// * `data` - the data to add
    fn add_receipt_data(&self, data: &[u8]) -> Result<(), ContextError>;

    /// add_event adds a new event to the execution result for this transaction.
    ///
    /// # Arguments
    ///
    /// * `event_type` -  This is used to subscribe to events. It should be globally unique and
    ///         describe what, in general, has occured.
    /// * `attributes` - Additional information about the event that is transparent to the
    ///          validator. Attributes can be used by subscribers to filter the type of events
    ///          they receive.
    /// * `data` - Additional information about the event that is opaque to the validator.
    fn add_event(
        &self,
        event_type: String,
        attributes: Vec<(String, String)>,
        data: &[u8],
    ) -> Result<(), ContextError>;

    fn get_sig_by_num(&self, block_num: u64) -> Result<String, ContextError>;

    fn get_reward_block_signatures(
        &self,
        block_id: &str,
        first_pred: u64,
        last_pred: u64,
    ) -> Result<Vec<String>, ContextError>;

    fn get_state_entries_by_prefix(
        &self,
        tip_id: &str,
        address: &str,
    ) -> Result<Vec<(String, Vec<u8>)>, ContextError>;
}

pub trait TransactionHandler {
    /// TransactionHandler that defines the business logic for a new transaction family.
    /// The family_name, family_versions, and namespaces functions are
    /// used by the processor to route processing requests to the handler.

    /// family_name should return the name of the transaction family that this
    /// handler can process, e.g. "intkey"
    fn family_name(&self) -> String;

    /// family_versions should return a list of versions this transaction
    /// family handler can process, e.g. ["1.0"]
    fn family_versions(&self) -> Vec<String>;

    /// namespaces should return a list containing all the handler's
    /// namespaces, e.g. ["abcdef"]
    fn namespaces(&self) -> Vec<String>;

    /// Apply is the single method where all the business logic for a
    /// transaction family is defined. The method will be called by the
    /// transaction processor upon receiving a TpProcessRequest that the
    /// handler understands and will pass in the TpProcessRequest and an
    /// initialized instance of the Context type.
    fn apply(
        &self,
        request: &TpProcessRequest,
        context: &mut dyn TransactionContext,
    ) -> Result<(), ApplyError>;
}
