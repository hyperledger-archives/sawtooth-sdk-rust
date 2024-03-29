/*
 * Copyright 2018 Bitwise IO, Inc.
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

#[macro_use]
extern crate cfg_if;

cfg_if! {
    if #[cfg(not(target_arch = "wasm32"))] {
        #[macro_use]
        extern crate clap;
        #[macro_use]
        extern crate log;

        use std::process;

        use log::LevelFilter;
        use log4rs::append::console::ConsoleAppender;
        use log4rs::config::{Appender, Config, Root};
        use log4rs::encode::pattern::PatternEncoder;

        use sawtooth_sdk::processor::TransactionProcessor;
        use sawtooth_xo::handler::XoTransactionHandler;
    } else {
        use sawtooth_xo::handler::apply;
        use sabre_sdk::{WasmPtr, execute_entrypoint};
    }
}

#[cfg(not(target_arch = "wasm32"))]
fn main() {
    let matches = clap_app!(xo =>
        (version: crate_version!())
        (about: "XO Transaction Processor (Rust)")
        (@arg connect: -C --connect +takes_value
         "connection endpoint for validator")
        (@arg verbose: -v --verbose +multiple
         "increase output verbosity"))
    .get_matches();

    let endpoint = matches
        .value_of("connect")
        .unwrap_or("tcp://localhost:4004");

    let console_log_level = match matches.occurrences_of("verbose") {
        0 => LevelFilter::Warn,
        1 => LevelFilter::Info,
        2 => LevelFilter::Debug,
        _ => LevelFilter::Trace,
    };

    let stdout = ConsoleAppender::builder()
        .encoder(Box::new(PatternEncoder::new(
            "{h({l:5.5})} | {({M}:{L}):20.20} | {m}{n}",
        )))
        .build();

    let config = match Config::builder()
        .appender(Appender::builder().build("stdout", Box::new(stdout)))
        .build(Root::builder().appender("stdout").build(console_log_level))
    {
        Ok(x) => x,
        Err(e) => {
            for err in e.errors().iter() {
                error!("Configuration error: {}", err.to_string());
            }
            process::exit(1);
        }
    };

    match log4rs::init_config(config) {
        Ok(_) => (),
        Err(e) => {
            error!("Configuration error: {}", e.to_string());
            process::exit(1);
        }
    }

    let handler = XoTransactionHandler::new();
    let mut processor = TransactionProcessor::new(endpoint);

    info!("Console logging level: {}", console_log_level);

    processor.add_handler(&handler);
    processor.start();
}

#[cfg(target_arch = "wasm32")]
fn main() {}

#[cfg(target_arch = "wasm32")]
#[no_mangle]
pub unsafe fn entrypoint(payload: WasmPtr, signer: WasmPtr, signature: WasmPtr) -> i32 {
    execute_entrypoint(payload, signer, signature, apply)
}
