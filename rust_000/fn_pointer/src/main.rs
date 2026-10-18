

fn plus_one(x: i32) -> i32 {
    x + 1
}


fn main() {
    let f1: fn(i32) -> i32;
    f1 = plus_one;
    println!(" {}", f1(10));

    let f2: fn(i32) -> i32 = plus_one;
    println!(" {}", f2(10));

    let f3: fn(i32) -> i32 = |x| x * 2;
    println!(" {}", f3(10));

    for (index, value) in (5..10).enumerate() {
	    println!("index = {} and value = {}", index, value);
    }
}