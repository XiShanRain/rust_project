fn main() {
    let s1 = String::from("hello");
    let s2 = s1; // s1 的所有权被移动到 s2，s1 不再有效
    // println!("{}", s1); // 编译错误，s1 已经失效

    let s3 = s2.clone(); // 使用 clone 方法创建 s2 的副本，s2 和 s3 都有效
    println!("s2 = {}, s3 = {}", s2, s3); // 输出
}
