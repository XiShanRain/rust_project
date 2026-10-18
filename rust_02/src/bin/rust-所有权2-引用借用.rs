fn calculate_length(s: &String) -> usize {
    // 使用引用传递参数，避免所有权转移
    s.len() // 返回字符串的长度
}
// 可变引用（同一作用域只能有一个）
fn push_str(s: &mut String, str_to_add: &str) {
    // 使用可变引用传递参数，允许修改字符串
    s.push_str(str_to_add); // 在原字符串末尾添加内容
}

fn main() {
    let mut s1 = String::from("hello");

    let length = calculate_length(&s1); // 传递 s1 的引用，避免所有权转移
    println!("s1 的长度为: {}", length);

    push_str(&mut s1, &" world");
    println!("s1 现在为: {}", s1);

    let r1 = &s1; // 不可变引用
    let r2 = &s1; // 另一个不可变引用
    // let r3 = &mut s1; // 可变引用，编译错误：不能同时存在可变引用和不可变引用
    println!("r1 = {}, r2 = {}", r1, r2);
}
