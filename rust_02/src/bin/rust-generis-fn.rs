use std::fmt::Display;
use std::cmp::PartialOrd;

fn main(){
    let a = 10;
    let b = 20;
    println!("{} 和 {} 中比较大的是: {}", a, b, larger(a, b));

    let x = 3.14;
    let y = 2.71;
    println!("{} 和 {} 中比较大的是: {}", x, y, larger(x, y));

    let c1 = 'a';
    let c2 = 'z';
    println!("{} 和 {} 中比较大的是: {}", c1, c2, larger(c1, c2));
}

fn larger<T: Display + PartialOrd + Copy>(a: T, b: T) -> T{
    if a >= b {
        a
    }else{
        b
    }
}
 
fn find_max<T: PartialOrd + Copy>(list: &[T]) -> Option<T> {
    let mut max = list.first()?;  // max 的类型是 &T
    for &item in &list[1..] {     // item 的类型是 T（解引用了）
        if item > *max {
            max = item;          
        }
    }
    Some(*max)
} 