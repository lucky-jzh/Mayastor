use common::MayastorTest;
use mayastor::{
    core::{Bdev, MayastorCliArgs},
    lvs::Lvs,
};
use rpc::mayastor::CreatePoolRequest;
use std::{ffi::CString, io::Error, mem::MaybeUninit};

pub mod common;

static DISKNAME1: &str = "/tmp/disk1.img";

fn get_allocated_blocks(path: &str) -> Result<i64, Error> {
    let mut data: MaybeUninit<libc::stat64> = MaybeUninit::uninit();
    let cpath = CString::new(path).unwrap();

    if unsafe { libc::stat64(cpath.as_ptr(), data.as_mut_ptr()) } < 0 {
        return Err(Error::last_os_error());
    }

    Ok(unsafe { data.assume_init().st_blocks })
}

#[tokio::test]
async fn aio_unmap_test() {
    common::delete_file(&[DISKNAME1.into()]);
    common::truncate_file(DISKNAME1, 64 * 1024);

    // Verify that there are currently no blocks allocated
    // for the sparse file that is our backing store.
    assert_eq!(get_allocated_blocks(DISKNAME1).unwrap(), 0);

    let args = MayastorCliArgs {
        reactor_mask: "0x3".into(),
        ..Default::default()
    };
    let ms = MayastorTest::new(args);

    // Create a pool.
    ms.spawn(async {
        Lvs::create_or_import(CreatePoolRequest {
            name: "tpool".into(),
            disks: vec!["aio:///tmp/disk1.img".into()],
        })
        .await
        .unwrap();
    })
    .await;

    // Check that we're able to find our new LVS.
    ms.spawn(async {
        assert_eq!(Lvs::iter().count(), 1);
        let pool = Lvs::lookup("tpool").unwrap();
        assert_eq!(pool.name(), "tpool");
        assert_eq!(pool.used(), 0);
        dbg!(pool.uuid());
        assert_eq!(pool.base_bdev().name(), "/tmp/disk1.img");
    })
    .await;

    // Create 4 lvols on this pool.
    ms.spawn(async {
        let pool = Lvs::lookup("tpool").unwrap();
        for i in 0 .. 4 {
            pool.create_lvol(&format!("vol-{}", i), 16 * 1024, true)
                .await
                .unwrap();
        }

        let pool = Lvs::lookup("tpool").unwrap();
        assert_eq!(pool.lvols().unwrap().count(), 4);
    })
    .await;

    // verify that some blocks have been allocated
    assert_ne!(get_allocated_blocks(DISKNAME1).unwrap(), 0);

    // Delete the lvols.
    ms.spawn(async {
        let pool = Lvs::lookup("tpool").unwrap();

        let f = pool
            .lvols()
            .unwrap()
            .map(|r| r.destroy())
            .collect::<Vec<_>>();

        assert_eq!(f.len(), 4);

        futures::future::join_all(f).await;
    })
    .await;

    // Destroy the pool
    ms.spawn(async {
        let pool = Lvs::lookup("tpool").unwrap();
        assert_eq!(pool.lvols().unwrap().count(), 0);

        pool.destroy().await.unwrap();
    })
    .await;

    // Validate the expected state of mayastor.
    ms.spawn(async {
        // pools destroyed
        assert_eq!(Lvs::iter().count(), 0);

        // no bdevs
        assert_eq!(Bdev::bdev_first().into_iter().count(), 0);
    })
    .await;

    // Verify that all used blocks have been discarded by confirming
    // that no blocks are currently allocated for the backing store.
    assert_eq!(get_allocated_blocks(DISKNAME1).unwrap(), 0);

    common::delete_file(&[DISKNAME1.into()]);
}
