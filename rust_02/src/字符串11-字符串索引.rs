fn main(){
    let s1 = String::from("hello+∞+❤️+Õ"); // 创建一个新的String类型，分配在堆上,它们在内存中以字节的形式存储，s1的长度是17个字节，但它包含了5个字符（h、e、l、l、o、+、∞、+、❤️、+、Õ），其中一些字符占用多个字节。
    let byte_slice = s1.as_bytes(); // 获取字符串s1的字节切片，指向堆上的数据,它的类型是&[u8]
    println!("Byte slice: {:?}", byte_slice); // 输出字节切片的内容
    //使用16进制打印
    for byte in byte_slice {
        print!("{:#X}\t", byte);
    }

    //字符串转换为字节向量
    let byte_vec = s1.into_bytes();
    println!("Byte vector: {:?}", byte_vec);
}