//! The suite against the boards that ship in `boards/`. The same daemon, three board files: the contract holds
//! for each, and only the file differs (the "only the install differs" guarantee, minus the install).

mod desktop {
    clawdot_device_conformance::conformance_tests!(clawdot_device_conformance::FileBoard::new(
        include_str!("../../../boards/desktop.json")
    ));
}

mod raspberry_pi {
    clawdot_device_conformance::conformance_tests!(clawdot_device_conformance::FileBoard::new(
        include_str!("../../../boards/raspberry-pi.json")
    ));
}

mod echo_dot2 {
    clawdot_device_conformance::conformance_tests!(clawdot_device_conformance::FileBoard::new(
        include_str!("../../../boards/echo-dot2.json")
    ));
}
