//! Socket activation: adopting the descriptor systemd passes in.

#[cfg(test)]
mod tests {
    use super::*;

    const ME: u32 = 4242;
    const NAME: &str = "org.tetsu.tx.v1";

    fn find_in(
        pid: Option<&str>,
        fds: Option<&str>,
        names: Option<&str>,
    ) -> Result<Option<i32>, ActivationError> {
        find(pid, fds, names, ME, NAME)
    }

    #[test]
    fn a_matching_name_gives_fd_three_plus_its_index() {
        assert_eq!(find_in(Some("4242"), Some("1"), Some(NAME)), Ok(Some(3)));
        assert_eq!(
            find_in(Some("4242"), Some("3"), Some("a:org.tetsu.tx.v1:b")),
            Ok(Some(4))
        );
    }

    #[test]
    fn another_pid_gives_none() {
        assert_eq!(find_in(Some("1"), Some("1"), Some(NAME)), Ok(None));
    }

    #[test]
    fn no_listen_variables_give_none() {
        assert_eq!(find_in(None, None, None), Ok(None));
        assert_eq!(find_in(Some("4242"), None, None), Ok(None));
    }

    #[test]
    fn an_absent_name_gives_none() {
        assert_eq!(find_in(Some("4242"), Some("2"), Some("a:b")), Ok(None));
        assert_eq!(find_in(Some("4242"), Some("1"), None), Ok(None));
    }

    #[test]
    fn a_malformed_fd_count_is_an_error() {
        assert!(find_in(Some("4242"), Some("many"), Some(NAME)).is_err());
        assert!(find_in(Some("4242"), Some("-1"), Some(NAME)).is_err());
    }

    #[test]
    fn more_names_than_descriptors_is_an_error() {
        assert!(find_in(Some("4242"), Some("1"), Some("a:b")).is_err());
    }
}
