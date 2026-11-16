#![expect(
    unsafe_code,
    reason = "PipeWire hands a DMA-BUF over as a bare descriptor number, and borrowing a number as a descriptor to duplicate it has no safe form"
)]

use std::os::fd::{BorrowedFd, OwnedFd};

use pipewire::spa::buffer::{Data, DataType};

pub(super) fn duplicate_dma_buf(data: &Data) -> Option<OwnedFd> {
    if data.type_() != DataType::DmaBuf {
        return None;
    }
    let number = data.fd();
    if number < 0 {
        return None;
    }
    // SAFETY: PipeWire keeps the descriptor of a DMA-BUF block open for as
    // long as the buffer that holds the block exists, and `data` borrows a
    // buffer that is dequeued for the whole call, so the number names an
    // open descriptor until the borrow made here ends on the next line.
    // https://docs.pipewire.org/page_dma_buf.html
    let borrowed = unsafe { BorrowedFd::borrow_raw(number) };
    borrowed.try_clone_to_owned().ok()
}
