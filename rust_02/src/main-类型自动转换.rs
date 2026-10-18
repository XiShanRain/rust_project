use std::any::type_name;

// 定义一个辅助函数，方便调用
fn type_of<T>(_: &T) -> &'static str {
    type_name::<T>()
}

fn main(){
    let num1 = 5;
    let num2 = 10;
    let sum: u8 = num1 + num2;
    println!("{}", sum);
    println!("{}", type_of(&num1)); //这里的&num1表示： num1 的引用
}