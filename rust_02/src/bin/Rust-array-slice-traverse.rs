fn type_of<T>(_: &T) -> String {
    format!("{}",std::any::type_name::<T>())
}
fn main(){
    println!("变量切片");
    let my_array: [i32; 5] = [1, 2, 3, 4, 5];
    let s1 = &my_array[1..=3]; // s1是一个不可变的切片。
    let mut sum = 0;
    for &i in s1 { //i的类型是i32
        println!("i type: {}", type_of(&i)); // 打印i的类型: i32
        sum += i; 
    }
    println!("sum is: {}", sum);
    //&i表示解引用切片s1中的元素。i的类型是i32，s1类型是&[i32]，&i32和&[i32]的关系是：&[i32]是一个切片类型，表示一个整数数组的连续部分，而&i32是一个引用类型，表示对一个整数的引用。
    //当我们使用for循环遍历切片s1时，每次迭代都会得到切片中的一个元素，这个元素的类型是i32。通过使用&符号，我们可以将这个元素解引用为一个整数值，并将其赋值给变量i。
}