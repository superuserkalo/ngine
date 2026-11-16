use std::ffi::c_void;
use std::fmt;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc::{self, Receiver, RecvTimeoutError, Sender, SyncSender};
use std::time::Duration;

use windows::Win32::Graphics::Direct3D11::{ID3D11Device, ID3D11Texture2D};
use windows::core::Interface;
use windows_capture::frame::Frame;

use crate::error::CaptureError;
use crate::frame::{CpuFrame, PixelFormat, Size};

const TIME_BETWEEN_LOOKS_AT_THE_STOP_FLAG: Duration = Duration::from_millis(10);

fn no_longer_lent() -> CaptureError {
    CaptureError::Unavailable(
        "the frame went back to the capture when the session stopped".to_owned(),
    )
}

type CopyRequest = SyncSender<Result<CpuFrame, CaptureError>>;

struct Loan {
    texture: ID3D11Texture2D,
    device: ID3D11Device,
    copy_requests: Sender<CopyRequest>,
}

/// A frame that is still in the capture's own texture.
///
/// Windows reuses that texture for the next frame, so capture waits while
/// this surface or a clone of it is alive. Drop it as soon as the encoder has
/// taken the frame. Once the session stops or restarts, the pixels can no
/// longer be copied to ordinary memory.
#[derive(Clone)]
pub struct D3d11Surface {
    loan: Arc<Loan>,
}

impl D3d11Surface {
    /// The `ID3D11Texture2D` pointer, never null. It stays valid while this
    /// surface or a clone of it is alive. Call `AddRef` on it to keep it for
    /// longer, which keeps the memory but not the picture.
    pub fn texture_ptr(&self) -> *mut c_void {
        self.loan.texture.as_raw()
    }

    /// The `ID3D11Device` that owns the texture, never null, with the same
    /// lifetime as [`D3d11Surface::texture_ptr`]. An encoder must use this
    /// device to read the texture.
    pub fn device_ptr(&self) -> *mut c_void {
        self.loan.device.as_raw()
    }

    /// The array slice of the texture that holds this frame.
    pub fn subresource_index(&self) -> u32 {
        0
    }

    pub(crate) fn to_cpu(&self) -> Result<CpuFrame, CaptureError> {
        let (request, copied) = mpsc::sync_channel(1);
        self.loan
            .copy_requests
            .send(request)
            .map_err(|_| no_longer_lent())?;
        copied.recv().map_err(|_| no_longer_lent())?
    }
}

impl fmt::Debug for D3d11Surface {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("D3d11Surface")
            .field("texture", &self.texture_ptr())
            .finish_non_exhaustive()
    }
}

pub(super) struct Lent {
    copy_requests: Receiver<CopyRequest>,
    size: Size,
    format: PixelFormat,
}

pub(super) fn lend(frame: &Frame, size: Size, format: PixelFormat) -> (D3d11Surface, Lent) {
    let (copy_requests, received) = mpsc::channel();
    let surface = D3d11Surface {
        loan: Arc::new(Loan {
            texture: frame.as_raw_texture().clone(),
            device: frame.device().clone(),
            copy_requests,
        }),
    };
    let lent = Lent {
        copy_requests: received,
        size,
        format,
    };
    (surface, lent)
}

fn copy(frame: &mut Frame, size: Size, format: PixelFormat) -> Result<CpuFrame, CaptureError> {
    let mut mapped = frame
        .buffer()
        .map_err(|error| CaptureError::Os(format!("the frame could not be read back: {error}")))?;
    let stride = usize::try_from(mapped.row_pitch()).ok();
    let pixels = mapped.as_raw_buffer().to_vec();
    stride
        .and_then(|stride| CpuFrame::new(size, format, stride, pixels))
        .ok_or_else(|| CaptureError::Os("the frame read back with an impossible size".to_owned()))
}

impl Lent {
    pub(super) fn serve_until_returned(self, frame: &mut Frame, capture_is_stopping: &AtomicBool) {
        loop {
            match self
                .copy_requests
                .recv_timeout(TIME_BETWEEN_LOOKS_AT_THE_STOP_FLAG)
            {
                Ok(request) => {
                    let requester_may_have_given_up =
                        request.send(copy(frame, self.size, self.format));
                    drop(requester_may_have_given_up);
                }
                Err(RecvTimeoutError::Disconnected) => return,
                Err(RecvTimeoutError::Timeout) => {
                    if capture_is_stopping.load(Ordering::Acquire) {
                        return;
                    }
                }
            }
        }
    }
}
