
// use std::cmp::Ordering;
use std::panic;
use std::sync::atomic::Ordering;
use std::sync::atomic::AtomicBool;
static FAIL_SAFE_MODE: AtomicBool = AtomicBool::new(false);
fn main() {
    //give custom panic hook
    panic::set_hook(
        Box::new(|info| {
            FAIL_SAFE_MODE.store(true, Ordering::SeqCst);
            println!("Panic occurred:{}", info);
            println!("Entering fail-safe mode.. .");
        })
    );

    // wrap the potentially panicking code in catch_unwind
    let result = panic::catch_unwind(|| {
        let buffer = [1, 2, 3, 4, 5];
        for i in 0..10 {
            // panics for i >=5
            println!("Accessing index {}:{}", i, buffer[i]);
        }
    });

    // Check if the opplication is in foil-safe mode
    if FAIL_SAFE_MODE.load(Ordering::SeqCst) {
        println!("System is now in fail-safe mode.");
    }
    // Printing result of the cotch
    match result {
        Ok(_) => println!("No panic occurred."),
        Err(_) => println!("Panic caught! Execution continues."),
    }
}
