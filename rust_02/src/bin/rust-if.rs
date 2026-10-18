fn main(){
    // let age = 10; //一个语句，以分号结尾
    // if age < 18 { //一个表达式，返回一个布尔值
    //     panic!("Sorry, you must be at least 18 years old to play this game.");
    // }else {
    //     println!("Welcome to the game!");
    // }

    let age = 10;
    let message = if age < 18 {
        print!("Sorry, you must be at least 18 years old to play this game.");
        10;
    } else {
        println!("Welcome to the game!");
        20;
    };
   
    println!("{:?}", message);
}