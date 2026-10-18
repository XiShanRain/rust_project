fn main(){
    let array1 = [1, 2, 3, 4, 5];

    // match array1 {
    //     // [1, 2, 3, 4, 5] => println!("Matched the entire array!"),
    //     [1, 2, 3, _, _] => println!("Matched the first three elements!"),
    //     [1, _, _, _, _] => println!("Matched the first element!"),
    //     _ => println!("No match!"),
    // }

    /* let invalid_array = match array1 {
      [n,_,_,_,_] | [_,n,_,_,_] | 
      [_,_,n,_,_] | [_,_,_,n,_] | [_,_,_,_,n] if n < 0 => {
        true
      }
        _ => false,
    }; */

    //使用matches!宏来检查数组是否包含任何负数元素
    let invalid_array = matches!(array1,
        [n,_,_,_,_] | [_,n,_,_,_] | 
        [_,_,n,_,_] | [_,_,_,n,_] | [_,_,_,_,n] if n < 0
    );
    if invalid_array == true {
        println!("Array is Invalid!");
    } else {
        println!("Array is valid!");
    }

    let some_value: Option<i32> = Some(5);

    // 使用 match
    match some_value {
        Some(x) => println!("Got value: {}", x),
        None => (),
    }

    // 使用 if let（更简洁）
    if let Some(x) = some_value {
        println!("Got value: {}", x);
    }
}