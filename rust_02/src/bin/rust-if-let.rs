fn main(){
    let some_value: Option<i32> = Some(5);

    // 使用 match
    match some_value {
        Some(x) => println!("Got value: {}", x),
        None => (),
    }

    // 使用 if let
    if let Some(x) = some_value {
        println!("Got value with if let: {}", x);
    } else {
        // None 的情况
    }

    let point = (4, 6);
    // 使用 match
    match point {
        (x, y) if x == y => println!("x and y are equal"),
        (x, y) if x + y == 10 => println!("x + y is 10"),
        (x, _) if x % 2 == 1 => println!("x is odd"),
        _ => println!("No condition matched"),
    }
    // 使用 if let
    if let (x, y) = point {
        if x == y {
            println!("x and y are equal");
        } else if x + y == 10 {
            println!("x + y is 10");
        } else if x % 2 == 1 {
            println!("x is odd");
        } else {
            println!("No condition matched");
        }
    }

    let point = (5, 5);
    if let (0, 0) = point {
        println!("Point is at the origin");
    } else {
        println!("Point is not at the origin");
    }

    let point = (5, 3);
    if let (_, y@1..4) = point {
        println!("y is between 1 and 4");
    } else {
        println!("y is not between 1 and 4");
    }
}