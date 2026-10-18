// mod module_1 { 
//     pub fn convert_kg_to_grams(in_kg: f64) -> f64 {
//         // let grams = in_kg * 1000 as f64; return grams;
//         in_kg * (1000 as f64) // 函数返回值,这是一个表达式，不是一条语句，因为没有以分号结尾
//     }
// }
 
// fn main(){
//     let result = module_1::convert_kg_to_grams(4 as f64);
//     println!("{}", result);
// }


fn concatenate_strings(str1: &str, str2: &str) -> String {
    // return str1.to_string() + str2;
    str1.to_string() + str2
}

fn find_biggest_string<'a>(str1: &'a str, str2: &'a str) -> &'a str {
    if str1.len() > str2.len() {
        return str1;
    } else {
        return str2;
    }
}
fn double_price(price: f64) -> f64 {
    let price = price * 2.0;
    println!("内部price=: {}", price);
    price
}

// 传递引用 修改原始值,将当前变量的内存地址传递给函数,传递的变量和函数内部的变量是同一个变量,修改函数内部的变量,也会修改外部变量的值
//星号表示解引用符,用于获取指针指向的值
fn double_price2(price: &mut f64) -> f64 {
    *price = *price * 2.0;
    println!("内部price2=: {}", price);
    *price
}

fn show_name(name: String) {
    println!("name=: {}", name);
}

fn main(){
    println!("{}", concatenate_strings("Hello", " World"));
    println!("{}", find_biggest_string("Hello", "World"));
 
    let price = 100.0;
    let new_price = double_price(price);
    println!("外部price=: {}, new_price=: {}", price, new_price);

    let mut price2 = 50.0;
    let new_price2 = double_price2(&mut price2);
    println!("外部price2=: {}, new_price2=: {}", price2, new_price2);

    let name = String::from("Alice");
    show_name(name);
    println!("{}", name);

}