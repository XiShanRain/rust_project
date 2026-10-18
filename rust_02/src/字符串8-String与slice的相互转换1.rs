fn main(){
    let s = String::from("Hello, Rust!"); // 创建一个新的String类型，分配在堆上
    let slice1 = s.as_str();
    let slice2 = &s;
    println!("s = {}", s);
    println!("slice1 = {}", slice1);
    println!("slice2 = {}", slice2);

    let slice = "hello";
    let string1 = slice.to_string();
    let string2 = String::from(slice);
    println!("slice = {}", slice);
    println!("string1 = {}", string1);
    println!("string2 = {}", string2);
}