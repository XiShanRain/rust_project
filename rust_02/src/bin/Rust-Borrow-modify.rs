//测试多个可变引用 修改 同一个可变引用变量
fn test_multiple_mutable_references_modify_same_mutable_variable(){ 
    let mut num1 = 50; // immutable reference
    let ref_of_num1 = &mut num1; // 第一次可变借用。
    let ref_of_num2 = &mut num1; // 第二次可变借用。

    *ref_of_num1 += 1;
    *ref_of_num2 = 200; // 通过可变引用 ref_of_num1 修改 num1 的值。
    //cannot borrow `num1` as mutable more than once at a time
    //Rust的借用规则规定，在同一作用域内，不能同时存在多个可变引用。这是为了防止数据竞争和不安全的内存访问。
    //当我们尝试创建第二个可变引用 ref_of_num2 时，编译器会报错，因为 num1 已经被 ref_of_num1 引用，不能再被修改。
    //即Rust中不允许对同一个引用对象持有多个可变借用。
}

//测试一个可变引用修改可变变量，一个不可变引用修改不可变变量
fn test_mutable_reference_modify_mutable_variable(){ 
    let mut num1 = 50; // 创建一个可变变量 num1，并赋值为 50。
    let ref_of_num1 = &mut num1; //num1已经变为可变借用
    let ref_of_num2 = &num1; //这里num1就不能再次作为不可变借用了
    *ref_of_num1 = 100;
    *ref_of_num2 = 200;

    //error[E0502]: cannot borrow `num1` as immutable because it is also borrowed as mutable
    let ref_of_num3 = &num1; //num1已经变为不可变借用
    let ref_of_num4 = & mut num1; //这里num1就不能再次作为可变借用了
    
}
fn main(){
    println!("借用、借用者与被借用者");
    // test_multiple_mutable_references_modify_same_mutable_variable();
    test_mutable_reference_modify_mutable_variable();
}