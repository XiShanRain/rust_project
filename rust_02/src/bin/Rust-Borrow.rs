// 可变引用修改可变引用变量
fn test_mutable_reference_modify_mutable_variable(){ 
     let mut num1 = 50; // 创建一个可变变量 num1，并赋值为 50。
    // let ref_of_num1 = &num1; // 创建一个不可变引用 ref_of_num1，指向 num1。
    // *ref_of_num1 = 100; // 错误：无法修改 num1，因为 ref_of_num1 是一个不可变引用。

    let ref_of_num1 = &mut num1; // 创建一个可变引用 ref_of_num1，指向 num1。
    *ref_of_num1 = 100; // 通过可变引用 ref_of_num1 修改 num1 的值。
    println!("num1 is: {}", num1); // 输出：num1 is: 100
}

// 可变引用修改不可变变量,
fn test_mutable_reference_modify_immutable_variable(){ 
    let num1 = 50;
    let ref_of_num1 = &mut num1; //这里不能为num1创建可变引用，因为num1是一个不可变变量。编译即报错
    *ref_of_num1 = 100; //发生错误 
    println!("num1 is: {}", num1);
}


fn main(){
    println!("借用、借用者与被借用者");
    // test_immutable_reference();
    test_mutable_reference_modify_mutable_variable();
}