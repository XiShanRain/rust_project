
fn main(){
    let file = std::fs::File::open("data.txt");
    match file {
        Ok(file)  => println!("打开成功: {file:?}"),
        Err(err) if err.kind() == io::ErrorKind::NotFound => println!("文件不存在"),
        Err(err)  => println!("其他错误: {err}"),
    }
}