impl Flag for Sort {
    fn update(&self, v: FlagValue, args: &mut LowArgs) -> anyhow::Result<()> {
        let kind = match convert::str(&v.unwrap_value())? {
            "none" => {
                args.sort = None;
                return Ok(());
            }
            "path" => SortModeKind::Path,
            "modified" => SortModeKind::LastModified,
            "accessed" => SortModeKind::LastAccessed,
            "created" => SortModeKind::Created,
            unk => anyhow::bail!("choice '{unk}' is unrecognized"),
        };
        args.sort = Some(SortMode { reverse: false, kind });
        Ok(())
    }
}

impl Flag for SortReverse {
    fn update(&self, v: FlagValue, args: &mut LowArgs) -> anyhow::Result<()> {
        let kind = match convert::str(&v.unwrap_value())? {
            "none" => {
                args.sort = None;
                return Ok(());
            }
            "path" => SortModeKind::Path,
            "modified" => SortModeKind::LastModified,
            "accessed" => SortModeKind::LastAccessed,
            "created" => SortModeKind::Created,
            unk => anyhow::bail!("choice '{unk}' is unrecognized"),
        };
        args.sort = Some(SortMode { reverse: true, kind });
        Ok(())
    }
}
