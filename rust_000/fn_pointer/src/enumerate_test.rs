fn main() {
    println!("1. 对范围-------------------");
    //1. 对范围
    for (index, value) in (5..10).enumerate() {
	    println!("index = {} and value = {}", index, value);
    }

    println!("2. 对向量-------------------");
    //2. 对向量
    let vec = vec![1, 2, 3, 4, 5];
    for (index, value) in vec.iter().enumerate() {
	    println!("index = {} and value = {}", index, value);
    }

    println!("3. 对字符串-------------------");
    //3. 对字符串
    let str = "hello\nworld";
    for (index, value) in str.chars().enumerate() {
	    println!("index = {} and value = {}", index, value);
    }

   
    let lines = "hello\nworld".lines();
    for (index, value) in lines.enumerate() {
        println!("index = {} and value = {}", index, value);
    }

     println!("4. 对数组-------------------");
    //4. 对数组
    let arr = [1, 2, 3, 4, 5];
    for (index, value) in arr.iter().enumerate() {
	    println!("index = {} and value = {}", index, value);
    }


    

}