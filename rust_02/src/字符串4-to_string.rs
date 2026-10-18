fn main(){
    let mut num_as_string = 3.14159.to_string(); // 使用to_string方法将浮点数转换为字符串
    num_as_string.push_str(" is a pretty long number."); // 使用push_str方法在字符串末尾添加更多文本
    num_as_string.insert_str(0, "PI = "); // 使用insert_str方法在字符串开头插入文本
    println!("{}", num_as_string);
}