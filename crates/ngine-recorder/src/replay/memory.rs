use super::backend::{Backend, StoredGroup};
use super::budget::BackendKind;
use super::error::ReplayError;
use super::packet::Packet;

#[derive(Debug, Default)]
pub(crate) struct MemoryBackend {
    open: Vec<Packet>,
}

impl Backend for MemoryBackend {
    fn kind(&self) -> BackendKind {
        BackendKind::Memory
    }

    fn open_group(&mut self) -> Result<(), ReplayError> {
        self.open.clear();
        Ok(())
    }

    fn append(&mut self, packet: &Packet) -> Result<(), ReplayError> {
        self.open.push(packet.clone());
        Ok(())
    }

    fn snapshot_open_group(&mut self) -> Result<StoredGroup, ReplayError> {
        Ok(StoredGroup::Memory(self.open.as_slice().into()))
    }

    fn seal_open_group(&mut self) -> Result<StoredGroup, ReplayError> {
        Ok(StoredGroup::Memory(std::mem::take(&mut self.open).into()))
    }

    fn discard_open_group(&mut self) {
        self.open = Vec::new();
    }
}
