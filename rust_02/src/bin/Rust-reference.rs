fn main() {
    let value = 42;
    println!("{}", value);
    //创建value的引用
    let ref_value = &value; //C语言中这个ref_value是一个指针,在Rust中它是一个引用，引用是一个安全的指针类型，编译器会确保它的使用不会导致悬垂指针或内存泄漏等问题。
    println!("{}", *ref_value);
    //value和ref_value都存储在栈中，所以它们都是可变变量。ref_value是一个引用，它指向value，而不是value本身。
    //value 被引用，ref_value是引用，它保存了value变量的地址。
    //ref_value是一个不可变引用，因为它没有使用mut关键字声明，所以我们不能通过ref_value来修改value的值。
    //如果我们尝试修改value的值，例如：value = 100; 这将导致编译错误，因为value是只读的。
    //value的类型是是 i32，这是整型变量。
    //ref_value的类型是 &i32，这是引用类型。
    println!("{}", ref_value); //打印：42，ref_value是一个引用，它指向value，所以当我们打印ref_value时，实际上是打印了value的值。
    //也可以称为自动解引用，Rust会自动解引用ref_value来获取它指向的值。
    //带星号表示“手动解引用”，不带星号表示“自动解引用”。

    //打印ref_value这个引用本身的地址
    println!("{:p}", ref_value); //打印： 0x15fcfc ，这是ref_value的指针地址。
    //ref_value是一个引用，它存储了value变量的地址，所以当我们打印ref_value时，实际上是打印了被引用者value的地址。
    println!("{:p}", &ref_value); //打印： 0x15fd00 ，这是ref_value这个引用本身的地址。
}