fn main() {
    // &str → String
    let s: String = "Hello".to_string();
    println!("地址1: {:p}", &s);
    let s: String = String::from("Hello");
    println!("地址2: {:p}", &s);
    // String → &str（借用）
    let string = String::from("Hello");
    let slice: &str = &string;        // 自动解引用
    let slice: &str = string.as_str(); // 显式转换

}