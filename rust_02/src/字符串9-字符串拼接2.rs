fn main(){
    let s1 = String::from("Good"); // 创建一个新的String类型，分配在堆上
    let s2 = " Morning".to_string(); // 通过to_string()方法将
    let s3 = s1 + " Evening"; // String + &str: 这里s1被移动了,所有权了转移给了s3，"Evening"是一个字符串切片,被借用了,所以s1不能再被使用了,s3是一个新的String类型，分配在堆上,
    println!("{}", s3);
}