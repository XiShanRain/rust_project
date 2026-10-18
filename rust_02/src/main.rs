fn takes_u32(num: u32) {
    println!("The number is: {}", num);
}

fn main() {
    let data: u8 = 255;
    takes_u32(data); // 错误：数据类型不匹配
    // takes_u32(data as u32);
}
