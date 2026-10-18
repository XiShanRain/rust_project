fn main() {
    let x = 5; //这里是整型
    let x = "Hello, Rust!"; // 这里是字符串类型
    println!("{}", x);

    // let mut y = 10; // 这里是整型
    // y = "Changed!"; // 这里是字符串类型，不能改变类型 expected integer, found `&str`
    // println!("{}", y);     

    //典型应用
    //1. 类型转换
    let input = "42";
    let input: i32 = input.parse().expect("Not a valid number!");
    println!("Parsed number: {}", input);

    //2. 逐步计算
    let price = 100.0;
    let price = price * 1.2; // 这里是浮点型
    let price = price + 10.0; // 这里是浮点型
    println!("Final price: {}", price);

    //3. 临时修改后恢复
    let message  = "Hello, Rust!";
    {
        let message = message.to_uppercase(); // 这里是字符串类型
        println!("内部 Uppercase message: {}", message);
    }
    println!("外部 Original message: {}", message);
}
