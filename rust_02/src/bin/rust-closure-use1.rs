fn main(){
    let double = |x| { x *2 };
    let add = |x, y| { x + y };
    let sum = add(10, 20);
    println!("Sum: {}", sum);
    let result = double(sum);
    println!("Result: {}", result);

    // 闭包可以捕获其环境中的变量
    let v = 3;
    let add2 = |x| { x + v };
    let result2 = add2(10);
    println!("Result2: {}", result2);

}