fn main(){
    let s1 = String::from("hello+∞+❤️+Õ"); // 创建一个新的String类型，分配在堆上
    s1.len(); // 获取字符串s1的长度，单位是字节
    println!("Length of s1 in bytes: {}", s1.len());
    let ch = s1.chars().nth(100); // 获取字符串s1中索引为3的字符，返回一个Option<char>类型
    println!("{:?}", ch);
    if let Some(c) = ch {
        println!("The character at index 1 is: {}", c);
    } else {
        println!("No character found at index 1.");
    }
}