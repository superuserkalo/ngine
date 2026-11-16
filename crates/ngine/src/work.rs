use std::io;
use std::sync::mpsc::{self, Receiver, TryRecvError};

pub enum Progress<T> {
    Running,
    Done(T),
    /// The thread ended without an answer.
    Lost,
}

/// Work on another thread whose answer the window's thread picks up when it
/// next looks.
pub struct Job<T> {
    answer: Receiver<T>,
}

impl<T: Send + 'static> Job<T> {
    pub fn spawn(name: &str, work: impl FnOnce() -> T + Send + 'static) -> io::Result<Self> {
        let (sender, answer) = mpsc::channel();
        std::thread::Builder::new()
            .name(name.to_owned())
            .spawn(move || {
                let _nobody_waits_any_more = sender.send(work());
            })?;
        Ok(Self { answer })
    }

    pub fn progress(&self) -> Progress<T> {
        match self.answer.try_recv() {
            Ok(answer) => Progress::Done(answer),
            Err(TryRecvError::Empty) => Progress::Running,
            Err(TryRecvError::Disconnected) => Progress::Lost,
        }
    }
}
