
fn main(){
//      //创建一个数组，rust中的数组是存储与栈上的
//    let a = [10,20,30];

//    let mut iter = a.iter(); // 从一个数组中返回迭代器
//    println!("{:?}",iter);

//    //使用 next() 方法返回迭代器中的下一个元素
//    println!("{:?}",iter.next());
//    println!("{:?}",iter.next());
//    println!("{:?}",iter.next());
//    println!("{:?}",iter.next());

    let v:Vec<&str> = vec!["a","b", "c"];
    let mut it:std::slice::Iter<'_, &str>  = v.iter();
    let s1:Option<&str> = it.next();
    println!("{:?}", s1);
    println!("{:?}", it.next());
    println!("{:?}", it.next());
    println!("{:?}", it.next());
}
