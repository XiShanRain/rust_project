fn main(){
    // Creating String using String literal
    let my_string = "Hello, Rust!".to_string(); // 使用to_string方法将字符串字面量转换为String类型
    println!("{}", my_string);

    let my_string = String::from("Hello, Rust!"); // 使用String::from函数创建一个新的String类型
    println!("{}", my_string);

    
}