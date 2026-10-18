use std::io::Write;
fn main(){
   // let mut line = String::new();
   // println!("请输入你的名字:");
   // let b1 = std::io::stdin().read_line(&mut line).unwrap();
   // println!("你输入的是: {}", line);
   // println!("读取的字节数为：{}", b1);
   // println!("调试字节: {:?}", line.as_bytes());

   let b1 = std::io::stdout().write("中国".as_bytes()).unwrap();
   println!("\n1_写入的字节数: {}", b1);
   let b2 = std::io::stdout().write(String::from("China").as_bytes()).unwrap();
   println!("\n2_写入的字节数: {}", b2);
   std::io::stdout().write(format!("\n3_写入的字节数为：{}\n",(b1+b2)).as_bytes()).unwrap();
}

// "rust": "cd $dir && rustc $fileName && $dir$fileNameWithoutExt",
