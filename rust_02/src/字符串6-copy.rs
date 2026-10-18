fn main() {
   // shallow copy of String 浅拷贝
    let s1 = String::from("Hello, Rust!"); // 创建一个新的String类型，分配在堆上
    let s2 = s1.clone(); // 这里发生了深度拷贝，s1和s2都拥有自己的数据副本
    println!("{}", s2); 
    println!("{}", s1); 

    // deep copy of String 深拷贝
    // let s1 = String::from("Hello, Rust!"); // 创建一个新的String类型，分配在堆上
    // let s2 = s1.clone(); // 这里发生了深度拷贝，s1和s2都拥有自己的数据副本
    // println!("{}", s2); // 输出字符串内容
    // println!("{}", s1); // 输出字符串内容
}
