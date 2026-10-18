use std::fs::File;
use std::io::Read; // File 没有引入 Read trait，所以编译器找不到 read_to_string 方法;std::fs::File 虽然实现了 Read，但必须显式引入 trait 才能调用这些方法

fn read_username_from_file() -> Result<String, std::io::Error> {
    let mut username = String::new();
    // 尝试从文件中读取用户名
    File::open("username.txt")?.read_to_string(&mut username)?;
    Ok(username)
}

fn main(){
    let username = read_username_from_file().unwrap();
    println!("用户名: {}", username);
}