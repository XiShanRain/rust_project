fn main() {
    let s1 = "Hello, Rust!"; // 不可变绑定 + 不可变数据(最严格)
    println!("{}", s1);
    let mut s2 = "Hello, Rust!"; // 可变绑定 + 不可变数据(严格)
    s2 = "Changed!"; // 重新赋值,用到了mut关键字
    println!("{}", s2);
    let s3 = String::from("Hello, Rust!"); // 不可变绑定 + 可变数据
    println!("{}", s3);
    let mut s4 = String::from("Hello, Rust!"); // 可变绑定 + 可变数据
    s4.push_str(" Welcome!"); // 修改内容,用到了mut关键字
    println!("{}", s4);
}