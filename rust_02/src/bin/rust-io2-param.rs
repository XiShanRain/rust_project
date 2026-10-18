
fn main(){
   let cmd_line = std::env::args();
   println!("总共有 {} 个命令行参数",cmd_line.len()); // 传递的参数个数
   for arg in cmd_line {
      println!("[{}]",arg); // 迭代输出命令行传递的参数
   }
}

