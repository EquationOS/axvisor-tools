#include <linux/io.h>
#include <linux/mm.h>
#include <linux/pgtable.h>

#include "includes/eqmanager.h"
#include "includes/hvc.h"
#include "includes/shm.h"
#include "includes/utils.h"

typedef struct eq_shm
{
	key_t key;			   // Shared memory key
	phys_addr_t base_gpa;  // Base physical address of the shared memory region
	size_t size;		   // Size of the shared memory region in bytes
	struct list_head list; // Linked list node
} eq_shm_t;

static LIST_HEAD(shm_list); // Head of the linked list

/// @brief Map the shared memory region into the user space process,
/// refer to `shmat()` in System V IPC.
/// `void *shmat(int shmid, const void *_Nullable shmaddr, int shmflg);`
/// @param file
/// @param vma
/// @param shmid equal to the shm key.
/// @return
int eq_shmat(struct file *file, struct vm_area_struct *vma, int shmid)
{
	eq_shm_t *shm = NULL;
	unsigned long pfn_start, mmap_size;
	int ret = 0;

	// Find the shared memory segment by its ID (shmid).
	list_for_each_entry(shm, &shm_list, list)
	{
		if (shmid == shm->key) // Assuming key is used as shmid for simplicity
		{
			break;
		}
	}

	if (!shm)
	{
		ERROR("Shared memory segment with ID %d not found\n", shmid);
		return -ENOENT;
	}

	mmap_size = vma->vm_end - vma->vm_start;
	pfn_start = shm->base_gpa >> PAGE_SHIFT; // Convert GPA to PFN

	if (mmap_size > shm->size)
	{
		ERROR(
			"Requested mmap size 0x%lx exceeds shared memory segment size "
			"0x%zx\n",
			mmap_size, shm->size);
		return -EINVAL;
	}

	INFO(
		"[%s] remap_pfn_range: va[0x%lx-0x%lx]\n", __func__, vma->vm_start,
		vma->vm_end);
	INFO(
		"Mapping SHM key 0x%x, base GPA 0x%llx, size 0x%zx\n", shmid,
		shm->base_gpa, shm->size);

	ret = remap_pfn_range(
		vma, vma->vm_start, pfn_start, mmap_size, vma->vm_page_prot);

	if (ret)
		ERROR(
			"%s: remap_pfn_range failed at [0x%lx  0x%lx]\n", __func__,
			vma->vm_start, vma->vm_end);

	return ret;
}

/// @brief shmget() returns the identifier of the System V shared memory
/// segment associated with the value of the argument key.  It may be
/// used either to obtain the identifier of a previously created
/// shared memory segment (when shmflg is zero and key does not have
/// the value IPC_PRIVATE), or to create a new set.
///
/// @param key the key of the shared memory segment.
/// @param size the size of the shared memory segment in bytes.
/// @param shmflg flags that control the operation:
/// * IPC_CREAT: create the segment if it does not already exist.
/// * IPC_EXCL: fail if the segment already exists.
/// * SHM_HUGETLB: use huge pages for the segment.
/// * SHM_NORESERVE: do not reserve swap space for the segment.
///
/// @return the shared memory identifier (shmid) on success, or -1 on failure
int eq_shmget(key_t key, size_t size, int shmflg)
{
	__u64 *shm_base;
	phys_addr_t shm_base_gpa;
	int ret;
	eq_shm_t *new_shm;

	shm_base = kmalloc(sizeof(__u64), GFP_KERNEL);
	if (!shm_base)
	{
		ERROR("Failed to allocate memory for shared memory base\n");
		return -ENOMEM;
	}

	shm_base_gpa = virt_to_phys(shm_base);

	INFO(
		"Creating shared memory with key 0x%x, size %zu, flags 0x%x\n", key,
		size, shmflg);

	// Hypercall to create or get the shared memory segment.
	ret = hvc_shmget(key, size, shmflg, shm_base_gpa);
	if (ret < 0)
	{
		ERROR(
			"Failed to create or get shared memory segment, error code: %d\n",
			ret);
		kfree(shm_base);
		return ret;
	}

	// Create a new shared memory segment entry.
	new_shm = kmalloc(sizeof(eq_shm_t), GFP_KERNEL);
	if (!new_shm)
	{
		ERROR("Failed to allocate memory for new shared memory entry\n");
		kfree(shm_base);
		return -ENOMEM;
	}
	new_shm->key = key;
	new_shm->base_gpa = shm_base_gpa;
	new_shm->size = size;
	INIT_LIST_HEAD(&new_shm->list);
	list_add_tail(&new_shm->list, &shm_list);
	INFO(
		"Shared memory segment created with key 0x%x, base GPA 0x%llx, size "
		"%zu\n",
		key, shm_base_gpa, size);

	return key;
}

/// @brief shmctl() performs control operations on the shared memory segment
/// identified by shmid. The command cmd specifies the operation to be
/// performed. The buf argument is a pointer to a structure that may be used to
/// pass additional information or to receive information about the segment.
///
/// @param shmid the identifier of the shared memory segment.
/// @param cmd the command to perform, such as IPC_RMID to remove the segment,
/// IPC_SET to set permissions, or IPC_STAT to get the segment's status.
/// @param buf a pointer to a structure that may be used to pass or receive
/// information about the segment.
///
/// @return 0 on success, or -1 on failure
int eq_shmctl(int shmid, int cmd, struct shmid_ds *buf) { return 0; }