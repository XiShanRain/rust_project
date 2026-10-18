use std::thread;
fn main() {
    // 创建两个线程并共享一个数据变量
    // 由于data是一个不可变的变量，所以它可以安全地在多个线程之间共享。
    let data: u8 = 255;

    // 在每个线程中访问共享数据
    let handle1 = thread::spawn(|| {
        println!("Thread 1 reads data = {}", data);
    });

    // 在第二个线程中访问共享数据
    let handle2 = thread::spawn(|| {
        println!("Thread 2 reads data = {}", data);
    });
    // 等待两个线程完成
    handle1.join().unwrap();
    handle2.join().unwrap();
}
