use std::io::Write; // 引入 Write trait 以使用 flush 方法

fn main() {
    println!("Hello, world!");
    std::io::stdout().flush().unwrap(); // 确保输出立即显示
    eprintln!("这是一个错误信息");
    let name = "Alice";
    let age = 18;
    let message = format!("{} 的年龄是 {}", name, age);
    println!("{}", message);

    let pi = 3.14;
    println!("圆周率的值是: {:.2}", pi); // 输出圆周

    //以十六进制打印十进制数
    let decimal_num = 6789;
    println!("十进制数: {},十六进制: {:X}", decimal_num, decimal_num);
    println!("十进制数: {},十六进制: {:#X}", decimal_num, decimal_num);
    println!("十进制数: {},十六进制: {:#x}", decimal_num, decimal_num);

    //以二进制打印十进制数,使用命名占位符
    println!("十进制数: {num},二进制: {num:b}", num = 6789);
    println!("十进制数: {},二进制: {:b}", decimal_num, decimal_num);

    //测试转义字符
    println!("David says \"Hello, World!\"");
    println!("c:\\Users\\David\\Documents");
    println!(r"c:\Users\David\Documents");
    println!(r#"\ \ \ "today is a good day" \ \ \"#);
}


