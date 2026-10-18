
fn read_config(path: &str) -> Result<String, std::io::Error> {
    // 尝试读取配置文件内容
    std::fs::read_to_string(path)
}

fn main(){
    let config_path = "config.txt";

    match read_config(config_path) {
        Ok(content) => println!("配置文件内容: {}", content),
        Err(e) => eprintln!("读取配置文件时出错: {}", e),
    }

    let content = read_config(config_path)
    .unwrap_or_else (|_| String::from("默认配置"));

    println!("配置内容: {}", content);
}