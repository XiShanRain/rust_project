fn find_max<T: PartialOrd + Copy>(list: &[T]) -> Option<T> {
    if list.is_empty() {
        return None;
    }

    let mut max = list[0];
    for &item in &list[1..] {
        if item > max {
            max = item;
        }
    }
    Some(max)
}

fn main() {
    let nums = vec![3, 1, 4, 1, 5, 9, 2, 6];
    match find_max(&nums) {
        Some(m) => println!("最大值是: {}", m),
        None => println!("列表为空"),
    }

    let floats = vec![3.14, 2.71, 1.41];
    println!("浮点数最大值: {}", find_max(&floats).unwrap());
}