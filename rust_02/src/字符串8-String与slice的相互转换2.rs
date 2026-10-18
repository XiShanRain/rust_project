fn print_string(s: &String){ // s 是一个字符串的引用
    println!("{}", s);
}

fn print_str(s: &str){ // s 是一个字符串切片的引用
    println!("{}", s);
}

fn main(){
    let s = String::from("Hello, Rust!"); // 创建一个新的String类型，分配在堆上
    let slice2 = &s; //这里slice2的类型是是 &String
    println!("{}", s);
    print_string(&s);
    print_str(slice2);

}