#![expect(
    unsafe_code,
    reason = "attaching to the terminal that started ngine is one Win32 call, and no safe crate in the tree offers it"
)]

const THE_PARENT_PROCESS: u32 = u32::MAX;

#[link(name = "kernel32")]
unsafe extern "system" {
    fn AttachConsole(process_id: u32) -> i32;
}

pub fn attach_to_the_parent() -> bool {
    // SAFETY: AttachConsole takes a process id or the constant that names
    // the parent, has no pointer arguments, and fails without side effects
    // when the parent has no console or this process already has one.
    let attached = unsafe { AttachConsole(THE_PARENT_PROCESS) };
    attached != 0
}
