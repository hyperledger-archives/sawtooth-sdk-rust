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
use std::collections::hash_map::Entry;
use std::collections::HashMap;
use std::str::from_utf8;

use sha2::{Digest, Sha512};

cfg_if! {
    if #[cfg(target_arch = "wasm32")] {
        use sabre_sdk::{ApplyError, TransactionContext};
    } else {
        use sawtooth_sdk::processor::handler::{ApplyError, TransactionContext};
    }
}

use crate::handler::game::Game;

pub fn get_xo_prefix() -> String {
    hex::encode(Sha512::digest(b"xo"))[..6].to_string()
}

pub struct XoState<'a> {
    context: &'a mut dyn TransactionContext,
    address_map: HashMap<String, Option<String>>,
}

impl<'a> XoState<'a> {
    pub fn new(context: &'a mut dyn TransactionContext) -> XoState {
        XoState {
            context,
            address_map: HashMap::new(),
        }
    }

    fn calculate_address(name: &str) -> String {
        let sha = hex::encode(Sha512::digest(name.as_bytes()))[64..].to_string();
        get_xo_prefix() + &sha
    }

    pub fn delete_game(&mut self, game_name: &str) -> Result<(), ApplyError> {
        let mut games = self._load_games(game_name)?;
        games.remove(game_name);
        if games.is_empty() {
            self._delete_game(game_name)?;
        } else {
            self._store_game(game_name, games)?;
        }
        Ok(())
    }

    pub fn set_game(&mut self, game_name: &str, g: Game) -> Result<(), ApplyError> {
        let mut games = self._load_games(game_name)?;
        games.insert(game_name.to_string(), g);
        self._store_game(game_name, games)?;
        Ok(())
    }

    pub fn get_game(&mut self, game_name: &str) -> Result<Option<Game>, ApplyError> {
        let games = self._load_games(game_name)?;
        if games.contains_key(game_name) {
            Ok(Some(games[game_name].clone()))
        } else {
            Ok(None)
        }
    }

    fn _store_game(
        &mut self,
        game_name: &str,
        games: HashMap<String, Game>,
    ) -> Result<(), ApplyError> {
        let address = XoState::calculate_address(game_name);
        let state_string = Game::serialize_games(games);
        self.address_map
            .insert(address.clone(), Some(state_string.clone()));
        self.context
            .set_state_entry(address, state_string.into_bytes())?;
        Ok(())
    }

    fn _delete_game(&mut self, game_name: &str) -> Result<(), ApplyError> {
        let address = XoState::calculate_address(game_name);
        if self.address_map.contains_key(&address) {
            self.address_map.insert(address.clone(), None);
        }
        self.context.delete_state_entry(&address)?;
        Ok(())
    }

    fn _load_games(&mut self, game_name: &str) -> Result<HashMap<String, Game>, ApplyError> {
        let address = XoState::calculate_address(game_name);

        Ok(match self.address_map.entry(address.clone()) {
            Entry::Occupied(entry) => match entry.get() {
                Some(addr) => Game::deserialize_games(addr).ok_or_else(|| {
                    ApplyError::InvalidTransaction("Invalid serialization of game state".into())
                })?,
                None => HashMap::new(),
            },
            Entry::Vacant(entry) => match self.context.get_state_entry(&address)? {
                Some(state_bytes) => {
                    let state_string = from_utf8(&state_bytes).map_err(|e| {
                        ApplyError::InvalidTransaction(format!(
                            "Invalid serialization of game state: {}",
                            e
                        ))
                    })?;

                    entry.insert(Some(state_string.to_string()));

                    Game::deserialize_games(state_string).ok_or_else(|| {
                        ApplyError::InvalidTransaction("Invalid serialization of game state".into())
                    })?
                }
                None => {
                    entry.insert(None);
                    HashMap::new()
                }
            },
        })
    }
}
