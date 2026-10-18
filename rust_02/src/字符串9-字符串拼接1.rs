fn main(){
    let mut s1 = String::new();
    s1.push_str("Good"); // 向字符串s1中追加字符串切片(到字符串末尾)

    let s2 = " Morning".to_string(); // 通过to_string()方法将字符串切片转换为字符串类型，分配在堆上
    // let s3 = s1 + s2; //String + String: error[E0308]: mismatched types ,s2 expected `&str`, found `String`
    let s3 = s1 + &s2; // String + &String: 这里s1被移动了，s2被借用了
    println!("{}", s3); // 输出 "GoodMorning"
    let s4 = s1 + " Rust!"; // String + &str: 这里s1被移动了，字符串切片被借用了
    println!("{}", s4); // 输出 "Good Rust!"
}