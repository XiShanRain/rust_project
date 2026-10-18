struct Employee {
    name: String,
    age: u8,
    company: String
}

impl Employee {
    fn show(&self) {
        println!("===> Name is: {} company is :{} age is {}", self.name, self.company, self.age);
    }
}

fn display(emp: Employee) {
    println!("Name is: {} company is :{} age is {}", emp.name, emp.company, emp.age);
}
fn main() {
    let emp1 = Employee {
        company: String::from("XCom"),
        name: String::from("Tom"),
        age: 30
    };
//   println!("Name is: {} company is :{} age is {}", emp1.name, emp1.company, emp1.age);
    display(emp1);

    let emp2 = Employee {
        company: String::from("YCom"),
        name: String::from("张三"),
        age: 30
    };
    emp2.show();
}

