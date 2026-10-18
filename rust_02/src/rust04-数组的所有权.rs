fn main() {
    let array1 = [5, 6, 7, 8]; //[i32; 4]
    let array2: [String; 3] = [
        String::from("foo"),
        String::from("bar"),
        String::from("baz"),
    ];

    // 基础类型的数组
    let item = array1[1]; // 获取数组中的元素,将索引为1 的元素复制到 item 中
    println!("{}", item);

    // 字符串数组
    /* let str = array2[1];
    println!("{}", str);   */

    let str: &str = &array2[1];
    println!("{}", str);

    for str in &array2 {
        println!("{}", str);
    }
    println!("{:?}", array2);

    let my_array = [1;10];
    println!("{:?}", my_array);
    // let my_array: [i32; 10] = [0;10];
}
