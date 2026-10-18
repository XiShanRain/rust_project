fn main(){
    let greeting = "Hello, world!+∞".to_string(); // 创建一个可变的String类型，分配在堆上
    let hello = &greeting[0..5]; // 获取字符串的切片，指向堆上的数据
    println!("{}", hello); // 输出 "Hello"

    let numbers = [1, 2, 3, 4, 5];
    let slice_of_numbers = &numbers[1..4]; // 获取数组的切片，指向栈上的数据
    println!("{:?}", slice_of_numbers);
    
    let mut numbers = [1, 2, 3, 4, 5];
    let slice_of_numbers = &mut numbers[1..4]; // 获取数组的切片，指向栈上的数据
    println!("{:?}", slice_of_numbers);
}