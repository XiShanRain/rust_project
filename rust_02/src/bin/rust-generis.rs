fn main(){
    let mut v:Vec<i32> = vec![1,2,3];
    // v.push("4"); //expected i32, found &'static str
    /*
        泛型结构体
        struct 名称<T> {
            字段: T,
        }
     */

    let g1:Data<i32> = Data {value: 100};
    println!("值:{}", g1.value);

    let g2:Data<f64> = Data {value: 123.45};
    println!("值:{}", g2.value);

    let book = Book {
        id: 100,
        name: String::from("张三"),
        author: String::from("张三")
    };
    book.show();
}

struct Data<T> {
    value: T,
}

struct Book {
    name: String,
    id: u32,
    author: String,
}

trait ShowBook {
    fn show(&self);
}

impl ShowBook for Book {
    fn show(&self){
        println!("id:{}, name:{}, author: {}",self.id, self.name, self.author);
    }
}
