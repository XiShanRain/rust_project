fn main() {
    //测试 heap clean up when String goes out of scope
    {
        let msg = String::from("Hello, Rust!"); // 创建一个新的String类型，分配在堆上
        println!("{}", msg); // 打印字符串内容
    } // msg goes out of scope here, and the heap memory is automatically cleaned up, 编译器会自动清理堆内存
    println!("{}", msg); // 这行代码会导致编译错误，因为msg已经超出作用域了
}
