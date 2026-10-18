
struct Foo<'a> {
    x: &'a i32,
}

impl<'a> Foo<'a> {
    fn x(&self) -> &'a i32 {
        self.x
    }
}

fn main() {
    let y = &5; // 等价于: let _temp = 5; let y = &_temp;
    let f = Foo { x: y }; // 等价于: let f = Foo::new(y);
    println!("{}", f.x());
}