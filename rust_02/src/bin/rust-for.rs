fn main() {
    //创建一个数组，rust中的数组是存储与栈上的
    let a: [i32; 3] = [10, 20, 30];
    for item in a {
        println!("{:?}\t", item);
    }
    // 获取数组的长度
    println!("数组的长度为: {}", a.len());

    //创建一个数组，rust中的数组是存储与栈上的
    let b: [&str; 3] = ["10", "20", "30"];
    let iter: std::slice::Iter<'_, &str> = b.iter(); // 从一个数组中返回迭代器
    println!("{:?}", iter);
    for item in iter { //这里的item类型是 &&str
        println!("{:?}\t", item);
    }
    // 获取数组的长度
    println!("数组的长度为: {}", b.len());

    
    //创建一个数组，rust中的数组是存储与栈上的
    let c: [String; 3] = [String::from("40"), String::from("50"), String::from("60")];
    let iter: std::slice::Iter<'_, String> = c.iter(); // 从一个数组中返回迭代器
    println!("{:?}", iter);
    for item in iter { //这里的item类型是 &String，而不是 String
        println!("{:?}\t", item);
    }
    // 获取数组的长度
    println!("数组的长度为: {}", c.len());

    //创建一个数组，rust中的数组是存储与栈上的
    let d: [String; 3] = [String::from("40"), String::from("50"), String::from("60")];
    println!("{:?}", d);
    for item in d {
        println!("{:?}\t", item);
    }
    // 获取数组的长度
    // println!("数组的长度为: {}", d.len()); //报错: borrow of moved value:d value borrowed here after move

    // &str实现了Copy
    let e: [&str; 3] = ["40", "50", "60"];
    println!("{:?}", e);
    for data in e {
        println!("{:?}\t", data);
    }
    // 获取数组的长度
    println!("数组的长度为: {}", e.len());
}
