use ngine_recorder::SoundFeed;

pub enum Speaker {}

impl Speaker {
    pub fn open(feed: SoundFeed) -> Option<Self> {
        drop(feed);
        None
    }
}
