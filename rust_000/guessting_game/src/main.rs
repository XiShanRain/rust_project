extern crate rand; // 在 【Cargo.toml】中声明了依赖项rand，使用extern crate引入rand库,等同于use rand;

use std::io;
use rand::Rng;
use std::cmp::Ordering;

fn main() {
    println!("猜测数字游戏!");
    let secret_number = rand::thread_rng().gen_range(1..101); // 生成1到100之间的随机数,gen_range()方法生成一个包含下限但不包含上限的随机数,所以这里是1到100之间的随机数
    println!("密码数字是: {}", secret_number);

    loop {
        println!("请输入你猜想的数字：");
        let mut guess = String::new();
        io::stdin()
            .read_line(&mut guess)
            .expect("无法读取行");

        let guess: u32 = match guess.trim().parse() {
            Ok(num) => num,
            Err(_) => continue,
        }.expect("请输入一个有效的数字");

        println!("你猜想的数字是：{}", guess);

        match guess.cmp(&secret_number) { // cmp()方法比较两个值的大小,返回一个Ordering枚举类型的值,表示两个值的大小关系
            Ordering::Less => println!("你猜想的数字太小了"),
            Ordering::Greater => println!("你猜想的数字太大了"),
            Ordering::Equal => {
                println!("你猜对了");
                break;
            },
        }
    }
}
