use core::num;

fn main() {
    // 获得一个 `i32` 类型的引用。`&` 表示取引用。
    let num = &100;

    match num {
        // 用 `&val` 这个模式去匹配 `num`，从而解构出引用内部的值。
        &val => println!("&val 是: {:?}", val),
    }

    // 如果不想在模式中使用 `&`，可以在匹配前手动解引用。
    match *num {
        // 这里的 val 直接绑定了 i32 的值。
        val => println!("val 是: {:?}", val),
    }

    // Rust 提供了 `ref` 关键字。它用于在模式匹配或绑定中，
    // 不移动（move）原数据，而是创建一个对原数据的引用。
    // 下面这行将得到一个 i32 的引用 (&i32)。
    let ref num3 = 66;
    match num3 {
        // 这里的 r 是 `&i32` 类型。
        r => println!("r 是: {:?}", r),
    }

    // 相应地，定义两个非引用的变量，通过 `ref` 和 `ref mut` 仍可取得其引用。
    let num4 = 5;
    let mut mut_num4 = 7;

    // 使用 `ref` 关键字来创建引用。
    // 下面的 r 是 `&i32` 类型。
    match num4 {
        ref r => println!("num4 r is: {:?}", r),
    }

    // 类似地使用 `ref mut`。
    match mut_num4 {
        ref mut m => {
            // 已经获得了 `mut_num4` 的可变引用，需要先解引用，才能改变它的值。
            *m += 10;
            println!("`mut_num4`: {:?}", m);
        }
    }
}