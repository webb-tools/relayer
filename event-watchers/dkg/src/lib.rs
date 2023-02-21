// Copyright 2022 Webb Technologies Inc.
//
// Licensed under the Apache License, Version 2.0 (the "License");
// you may not use this file except in compliance with the License.
// You may obtain a copy of the License at
//
// http://www.apache.org/licenses/LICENSE-2.0
//
// Unless required by applicable law or agreed to in writing, software
// distributed under the License is distributed on an "AS IS" BASIS,
// WITHOUT WARRANTIES OR CONDITIONS OF ANY KIND, either express or implied.
// See the License for the specific language governing permissions and
// limitations under the License.

/// A module for listening on proposal events.
mod proposal_handler_watcher;
#[doc(hidden)]
pub use proposal_handler_watcher::*;
/// A module for listening on DKG Governor Changes event.
mod governor_watcher;
#[doc(hidden)]
pub use governor_watcher::*;

/// A module for listening on DKG Metadata pallet events.
mod dkg_metadata;
#[doc(hidden)]
pub use dkg_metadata::*;
