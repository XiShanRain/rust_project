// 基本函数
fn add(x: i32, y: i32) -> i32 {
    x + y // 最后一个表达式作为返回值（无分号）
}
// 无返回值（返回单元类型 ()）
fn greet(name: &str) {
    println!("Hello, {}!", name);
}
// 提前返回
fn abs(x: i32) -> i32 {
    if x < 0 {
        return -x;
    }
    x
}
fn main() {
    let result = add(3, 5);
    println!("3 + 5 = {}", result);
    greet("Rust");
}
