fn main() {
    //创建一个新的可变的字符串
    //变量greeting的类型是String，String类型是一个可变的、可增长的字符串类型，存储在堆上，可以动态地增加或减少长度。
    let mut greeting = String::from("Good morning!");
    println!("{}", greeting);

    //替换"morning"为"afternoon"
    greeting = greeting.replace("morning", "afternoon");
    println!("replace={}", greeting);

    //使用replace_range方法替换字符串的一部分
    greeting.replace_range(5.., "evening");
    println!("replace_range={}", greeting);

    greeting.push_str("Have a nice day!"); // 使用push_str方法在字符串末尾添加更多文本
    println!("push_str={}", greeting); //打印: Good evening! Have a nice day!
}
