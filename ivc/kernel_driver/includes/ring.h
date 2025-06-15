#pragma once

#include <stdatomic.h>
#include <stddef.h>

/**
 *
|---------------------------|
| struct shm_ring_t         |
|---------------------------|
| data buffer               |
|---------------------------|
 */
typedef struct
{
	uint64_t publisher_id; // channel publisher ID
	uint64_t key;		   // channel key
	size_t size;
	size_t head;		// index of the head slot
	size_t tail;		// index of the tail slot
	size_t data_offset; // offset of the data buffer
} shm_ring_t;

void shm_ring_init(
	void *shm_base, size_t shm_region_size, uint64_t channel_key);
size_t shm_ring_enqueue(void *base, const char __user *data, size_t len);
int shm_ring_dequeue(
	void *base, char __user *buf, size_t count, size_t *out_len);
