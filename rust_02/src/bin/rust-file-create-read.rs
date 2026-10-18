use std::io::{Read, Write};


fn main(){
    let mut file = std::fs::File::create("data.txt").expect("创建文件失败!");
    file.write_all("中国人民".as_bytes()).expect("写入失败!");
    file.write_all("中华民族".as_bytes()).expect("写入失败");

    let mut file = std::fs::File::open("data.txt").unwrap();
    let mut content = String::new();
    file.read_to_string(&mut content).unwrap();
    println!("{}", content)
}