fn main() {
     let s = String::from("hello");
    
    // let r1 = &s;           // 这是借用：不可变借用
    // let r2 = &mut s;       // 错误示例：s 不可变，这里会编译失败
    // println!("{}", r2);

     let mut s2 = String::from("world");
    let r3 = &mut s2;      // 这是借用：可变借用
    r3.push_str(",!");
    println!("{}", s2);
    // println!("{}", r3);
}