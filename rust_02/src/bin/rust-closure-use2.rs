//返回闭包函数
fn return_closure() -> impl Fn(i32, i32) -> i32 {
    |x, y| x + y
}

fn main() {
    let closure = return_closure();
    let result = closure(10, 20);
    println!("Result from returned closure: {}", result);
}
