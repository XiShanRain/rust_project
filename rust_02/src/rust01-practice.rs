/*
    编写一个程序，用于检查用户输入的密码是否合法。密码的检查标准如下：
    1. 至少包含 1 个小写字母 [a-z]；
    2. 至少包含 1 个数字 [0-9]；
    3. 至少包含 1 个大写字母 [A-Z]；
    4. 至少包含 1 个特殊字符 [$#@]；
    5. 密码长度最小为 6 位；
    6. 密码长度最大为 12 位。
*/

//期望输出
/*
    ####### 设置新密码 #######
    1. 至少包含1个字母（a-z）
    2. 至少包含1个数字（0-9）
    3. 至少包含1个大写字母（A-Z）
    4. 至少包含1个特殊字符（#@）
    5. 交易密码最小长度：6位
    6. 交易密码最大长度：12位

    输入新密码： <用户输入密码>
    再次输入新密码： <用户再次输入相同密码>
    密码强度：弱/强/不匹配
    密码修改成功/失败
 */

fn is_password_match(password: &str, confirm_password: &str) -> bool {
    /* if password == confirm_password {
        return true;
    }
    false */
    password == confirm_password // 直接返回比较结果,因为这是一个布尔表达式,表达式的结果就是函数的返回值
}

fn is_password_contain_lower_case_letters(password: &str) -> bool {
    // 创建一个布尔变量,用于记录密码是否包含小写字母
    let mut contains_lower_case_letters: bool = false;

    // 遍历密码中的每一个字符
    for character in password.chars() {
        // 判断字符是否为小写字母
        /* if character >= 'a' && character <= 'z' {
            contains_lower_case_letters = true;
            break;
        } */
        if character.is_ascii_lowercase() {
            contains_lower_case_letters = true;
            break;
        }
    }
    contains_lower_case_letters
}

fn is_password_contain_upper_case_letters(password: &str) -> bool {
    // 创建一个布尔变量,用于记录密码是否包含大写字母
    let mut contains_upper_case_letters: bool = false;

    // 遍历密码中的每一个字符
    for character in password.chars() {
        // 判断字符是否为大写字母
        if character.is_ascii_uppercase() {
            return true;
            break;
        }
    }
    false
}

fn check_password_strength(password: &str) -> i32 {
    //weak = 0, strong = 1, not_match = 2
    let contains_lower_case_letters: bool = is_password_contain_lower_case_letters(password); // 包含小写字母
    let contains_upper_case_letters: bool = false; // 包含大写字母
    let contains_digits: bool = false; // 包含数字
    let contains_special_characters: bool = false; // 包含特殊字符
    let contains_not_allowed_special_characters: bool = true; // 存在不允许的特殊字符
    let mut strength: i32 = 0;

    if contains_lower_case_letters &&
        is_password_contain_upper_case_letters(password) &&
        contains_digits &&
        contains_special_characters &&
        !contains_not_allowed_special_characters {
        strength = 1;
    } else {
        strength = 0;
    }
    strength
}

#[cfg(test)]
mod tests {
    use crate::*;

    #[test]
    fn test_when_both_the_password_are_same_fn_return_true() {
        assert_eq!(true, is_password_match("123456", "123456"));
    }

    #[test]
    fn test_when_both_the_password_are_different_fn_return_false() {
        assert_eq!(false, is_password_match("123456", "1234567"));
    }

    mod password_strength {
        use super::*;

        #[test]
        fn test_when_password_is_empty_fn_return_false() {
            assert_eq!(0, check_password_strength(""));
        }


        #[test]
        fn test_when_password_contains_only_a_to_z_letters_fn_return_false() {
            assert_eq!(0, check_password_strength("abcdefghijklmnopqrstuvwxyz"));
        }
        

        #[test]
        fn test_when_password_contains_only_A_to_Z_letters_fn_return_false() {
            assert_eq!(0, check_password_strength("ABCDEFGHIJKLMNOPQRSTUVWXYZ"));
        }

        #[test]
        fn test_when_password_contains_only_digits_fn_return_false() {
            assert_eq!(0, check_password_strength("1234567890"));
        }

        #[test]
        fn test_when_password_length_is_less_than_6_characters_fn_return_false() {
            assert_eq!(0, check_password_strength("12345"));
        }

        #[test]
        fn test_when_password_length_is_greater_than_12_characters_fn_return_false() {
            assert_eq!(0, check_password_strength("1234567890123"));
        }

        //测试密码是否包含小写字母
        #[test]
        fn test_when_password_contains_lower_case_letters_fn_return_true() {
            assert_eq!(true, is_password_contain_lower_case_letters("Abcdefg"));
        }

        //测试密码是否包含大写字母
        #[test]
        fn test_when_password_contains_upper_case_letters_fn_return_true() {
            assert_eq!(true, is_password_contain_upper_case_letters("12345A"));
        }

        //测试密码没有大写字母返回false
        #[test]
        fn test_when_password_does_not_contain_upper_case_letters_fn_return_false() {
            assert_eq!(false, is_password_contain_upper_case_letters("12345a"));
        }

    }
}