fn main(){
    let s1 = String::from("Good"); // 创建一个新的String类型，分配在堆上
    let s2 = " Morning".to_string(); // 通过to_string()方法将
    let s3 = format!("{} {}", s1, s2); // 使用format!宏来格式化字符串，s1和s2被借用了，s3是一个新的String类型，分配在堆上
    println!("{}", s1);
    println!("{}", s3);

    let s4 = "Good" + " Night"; // &str + &str: 这里"Good"是一个字符串切片,被借用了,"Night"也是一个字符串切片,被借用了,s4是一个新的String类型，分配在堆上
    println!("{}", s4);
}