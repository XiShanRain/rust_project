fn main() {
    // let mut s1 = "Hello, Rust!"; // 不可变绑定 + 不可变数据(最严格)
    // // s1 = "Changed!"; // 重新赋值,用到了mut关键字
    // s1.push_str(" Welcome!");  
    // println!("{}", s1);

    let mut s2 = String::from("Hello, Rust!"); // 不可变绑定 + 可变数据
    s2.push_str(" Welcome!"); // 修改内容,用到了mut关键字
    println!("{}", s2);
}