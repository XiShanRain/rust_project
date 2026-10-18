fn main(){
    // let my_array: [i32; 5] = [-56, 22, 3, 444, 90]; // 定义一个数组，包含5个整数元素。
    // let s1 = &my_array[1..=3]; // s1是一个切片，它的类型是 &[i32]，表示一个只读的整数切片。
    // println!("s1 is: {:?}", s1); // 输出切片s1的内容
    // println!("s2 is: {:?}", s1[0]);
    // println!("s2 is: {:?}", s1[2]);
    // println!("s2 is: {:?}", s1[5]); //会导致运行时恐慌，因为s1只有3个元素，索引5超出了切片的范围。
    
    //修改数组
    // let my_array: [i32; 5] = [-56, 22, 3, 444, 90]; // 定义一个数组，包含5个整数元素。
    // let s1 = &my_array[1..=3]; // s1是一个切片，它的类型是 &[i32]，表示一个只读的整数切片。
    // s1[2] = 100; // 错误：无法修改切片s1，因为s1是一个不可变引用。 error[E0594]: cannot assign to `s1[_]`, which is behind a `&` reference


    // let mut my_array: [i32; 5] = [-56, 22, 3, 444, 90];
    // let s1 = &my_array[1..=3]; //&my_array是一个不可变引用
    // s1[2] = 100; // 错误：无法修改切片s1，因为s1是一个不可变引用。 error[E0594]: cannot assign to `s1[_]`, which is behind a `&` reference

    //改为可变数组
    let mut my_array: [i32; 5] = [-56, 22, 3, 444, 90]; 
    //改为可变引用，切片s1的类型是：&mut [i32]
    let s1 = &mut my_array[1..=3]; 
    s1[2] = 100; //第二个元素是444
    println!("{:?}", my_array)
}