use std::error::Error;
use std::fmt;
use std::path::PathBuf;

use vik_workflow::{
    Diagnose, Diagnoses, DiagnosisSeverity, SystemDiagnoseEnvironment, parse_workflow_file,
    select_workflow_path,
};

use crate::env;

#[derive(Debug)]
struct DoctorFailed;

impl fmt::Display for DoctorFailed {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str("doctor found failing diagnostics")
    }
}

impl Error for DoctorFailed {}

pub(crate) fn run(workflow: Option<PathBuf>) -> Result<(), Box<dyn Error>> {
    env::load_dotenv()?;
    let path = select_workflow_path(workflow);
    let environment = SystemDiagnoseEnvironment;
    let diagnoses = match parse_workflow_file(&path) {
        Ok(definition) => definition.diagnose(&environment),
        Err(err) => {
            let mut diagnoses = Diagnoses::new();
            diagnoses.push(vik_workflow::Diagnosis::error(
                "config.workflow",
                format!("workflow could not be loaded: {err}"),
            ));
            diagnoses
        }
    };

    print_diagnoses(&path, &diagnoses);
    if diagnoses.has_errors() {
        Err(Box::new(DoctorFailed))
    } else {
        Ok(())
    }
}

fn print_diagnoses(path: &std::path::Path, diagnoses: &Diagnoses) {
    println!("workflow doctor: {}", path.display());
    for diagnosis in diagnoses.iter() {
        println!(
            "[{}] {}: {}",
            diagnosis.severity.label(),
            diagnosis.name,
            diagnosis.message
        );
    }
    println!(
        "doctor summary: {} ok, {} warnings, {} errors",
        diagnoses.count(DiagnosisSeverity::Passed),
        diagnoses.count(DiagnosisSeverity::Warning),
        diagnoses.count(DiagnosisSeverity::Error)
    );
}
