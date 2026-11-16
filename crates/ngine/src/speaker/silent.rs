use std::sync::Arc;
use std::sync::atomic::AtomicU8;

use ngine_recorder::SoundFeed;

pub enum Speaker {}

impl Speaker {
    pub fn open(feed: SoundFeed, volume_percent: Arc<AtomicU8>) -> Option<Self> {
        drop((feed, volume_percent));
        None
    }
}
