fn main() {
    // loop —— 无限循环
    let mut count = 0;
    let result = loop {
        count += 1;
        if count == 10 {
            break count * 2; // break 可以返回值
        }
    };
    println!("循环结束，结果为: {}", result);
    
    // while 循环
    let mut n = 3;
    while n > 0 {
        println!("{}!", n);
        n -= 1;
    }
    // for 循环（推荐方式）
    let arr = [10, 20, 30, 40, 50];
    for element in arr.iter() {
        println!("值: {}", element);
    }
    // 范围迭代
    for i in 0..5 {
        println!("i = {}", i); // 0, 1, 2, 3, 4
    }
    for i in (1..=3).rev() {
        println!("倒数: {}", i); // 3, 2, 1
    }
}
