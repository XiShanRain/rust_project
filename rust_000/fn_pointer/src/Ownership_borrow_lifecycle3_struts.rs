
struct Foo<'a> {
    x: &'a i32,
}


fn main() {
    let y = &5;
    let f = Foo { x: y };
    println!("{}", f.x);

    //上面的展开类似下面的
    let _temp = 5;
    let y = &_temp;

    let f = Foo {
        x: y
    };

    println!("{}", f.x);

    // _temp 活得最长
    // y 是指向 _temp 的引用
    // f.x 保存了 y
}