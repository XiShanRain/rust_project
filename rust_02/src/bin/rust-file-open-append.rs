use std::io::{Read, Write};
use std::fs::OpenOptions;

fn main(){
    let mut file = OpenOptions::new().append(true).open("data.txt").expect("无法打开文件");
    file.write_all("\n湖光秋月两相和".as_bytes()).expect("写入失败!");
    file.write_all("\n潭面无风镜未磨".as_bytes()).expect("写入失败");
}