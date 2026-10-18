use std::io;
/*
 Convert senconds to HH:MM:SS format
*/

fn main(){
    let mut input = String::new();
    println!("Enter the time of the day in seconds:(0 to 86,399):");
    io::stdin()
    .read_line(&mut input)
    .expect("Failed to read line");
    //.unwrap(); // 读取用户输入并处理错误

    let total_seconds: i32 = input
                            .trim()
                            .parse()
                            .expect("Input number only without any sign!"); // 将输入转换为整数并处理错误

    if total_seconds < 0 || total_seconds > 86399 {
        panic!("Please enter a valid number between 0 and 86,399.");
    }        

    let hours = total_seconds / 3600;
    let remaining_seconds = total_seconds % 3600;
    let minutes = remaining_seconds / 60;
    let seconds = remaining_seconds % 60;

    println!("Time in 24hr format is： {:02}:{:02}:{:02}", hours, minutes, seconds);             
}