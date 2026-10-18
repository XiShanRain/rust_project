fn main(){
    let x = 5;
    let y = 10;
    let z = x == y; // 这是一个比较表达式，结果是一个布尔值
    println!("z is {}", z);
    let z= y > x; // 这是一个比较表达式，结果是一个布尔值
    println!("z is {}", z);

    if x == y {
        println!("x and y are equal");
    } else if x > y {
        println!("x is greater than y");
    } else {
        println!("x is less than y");
    }
}