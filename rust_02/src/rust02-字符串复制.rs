fn main() {
    //s1·is·the·owner·of-the·string. "hello"
    let s1 = String::from("hello");
    //Ownership·of.'sl'·is·transferred-to 's"
    //'funl' returns-ownership-of- 's'·back-to-s2
    let s2 = fun1(s1); //s1 is no longer valid,because ownership has been transferred to fun1, and fun1 returns ownership to s2

    println!("{}", s2); //OK
    println!("{}", s1); //Error
}

fn fun1(mut s: String) -> String {
    //s是可变的引用，函数参数s的所有权被转移到fun1函数中，s的所有权被fun1函数返回给调用者
    println!("{ }", s); 
    s.push_str(" World");
    s //ownership returned,因为s在栈中所以会被自动释放
}
