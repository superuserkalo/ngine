use std::ffi::c_void;

use crate::error::CaptureError;
use crate::frame::CpuFrame;

#[derive(Clone, Copy, Debug)]
enum NoBackendYet {}

#[derive(Clone, Debug)]
pub struct D3d11Surface {
    texture: NoBackendYet,
}

impl D3d11Surface {
    /// The `ID3D11Texture2D` pointer, never null. It stays valid while this
    /// surface or a clone of it is alive. Call `AddRef` on it to keep it for
    /// longer.
    pub fn texture_ptr(&self) -> *mut c_void {
        match self.texture {}
    }

    /// The array slice of the texture that holds this frame.
    pub fn subresource_index(&self) -> u32 {
        match self.texture {}
    }

    pub(crate) fn to_cpu(&self) -> Result<CpuFrame, CaptureError> {
        match self.texture {}
    }
}
