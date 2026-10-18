// import "main-类型自动转换";

fn main(){
    // let num1 = 20; //相当于 const 常量
    // let num2 = 4.5;
    // let mult = num1 * (num2 as i32);
    // println!("{}", mult);
    // let num3: &i32 = &num1;
    // let mult2 = num1 * num3;
    // println!("{}", mult2);


    // let my_array: [f64,4] = [1.0, 2.5, 3.5];

    // let my_array: [char; 4] = ['a', 'b', 'c', 'd'];
    // println!("{:?}", my_array);

    // let num1 = 5;
    // println!("{}", num1); //正常
    // let my_array = [1, 2, 3_u8, 4, 5]; //这个数组的类型是 [u8; 5]
    // println!("{:}", my_array);//错误


    // let my_array = [0;10];
    // let my_array: [i32; 10] = [0;10];

    //数组索引
    // let my_array = [5, 4, 3, 2, 1];
    // println!("{}", my_array[3]);
    // //my_array[3] = 10;//错误，不能修改数组，因为默认是不可变的

    // let mut my_array = [5, 4, 3, 2, 1];
    // my_array[3] = 10;
    // println!("{:?}", my_array);

    //char类型的标量值
    // let infinity_symbol = '\u{221E}';
    // println!("symbol = {}, usv = {}", infinity_symbol, infinity_symbol as u32);

    // let tm = '®'; //这是一个char字面量
    // println!("{:X}", tm as i32 ); //打印char的Unicode的标量值QUSV

    // let tm = '\u{00AE}'; //告诉rust编译器，要存储一个字符类型的变量，可以存储单个字符。
    // println!("{}", tm); //打印 `®`

    // let usv_of_inf = 0x221E_u32;
    // println!("symbol = {}", usv_of_inf);//打印： symbol = 8734
    // println!("symbol = {}", usv_of_inf as char);//Error: invalid cast，因为0x221E_u32无法转换为char类型，char 类型表示 Unicode 标量值，其值范围从 U+0000 到 U+10FFFF。然而，并非所有 u32 值都是有效的 char 值。

    //要转换一个u32值到一个char类型，可以使用char::from_u32()函数,它返回一个Option<char>
    // let usv_of_inf = 0x221E_u32;
    // if let Some(char_value) = char::from_u32(usv_of_inf) {
    //     println!("symbol = {}", char_value);
    // }else{
    //     println!("{}", "不是一个有效的Unicode标量值，无法转换");
    // }


    //循环数组
    // let my_array = [5, 4, 3, 2, 1];
    // for i in my_array.iter(){
    //     println!("数组值：{}", i);
    // }

    // for i in 0..my_array.len(){
    //     println!("{}", my_array[i]);
    // }

    // let mut sum = 0;
    // for i in my_array {
    //     sum += i;
    // }
    // println!("{}", sum);
    

    //算数运算符
    // let num1 = 15_u8;
    // let num2 = 10_u8;
    // let sum = num1 / num2;
    // println!("{}", sum);

    //函数
    fn sum(num1: i32, num2: i32) -> i32 {
        num1 + num2
    }
}