fn main() {
    // The generated UI needs more stack than Windows gives a process main thread
    // in release builds. Compile it on a thread with an explicit stack size.
    std::thread::Builder::new()
        .stack_size(32 * 1024 * 1024)
        .spawn(|| slint_build::compile("ui/app.slint"))
        .expect("failed to start Slint UI compilation")
        .join()
        .expect("Slint UI compilation panicked")
        .expect("failed to compile Slint UI");
}
