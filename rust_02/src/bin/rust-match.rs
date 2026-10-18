fn main() {
    let x = 1;
    match x {
        1 => println!("one"),
        2 => println!("two"),
        3 => println!("three"),
        _ => println!("something else"), // _ 是一个通配符，表示匹配所有其他情况
    }

    let x = 5;
    match x {
        1 => { println!("one") }
        2 => { println!("two") }
        3 => { println!("three") }
        _ => { println!("something else") } // _ 是一个通配符，表示匹配所有其他情况
    }
}
