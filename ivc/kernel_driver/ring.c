#include <linux/io.h>
#include <linux/uaccess.h>

#include "includes/ring.h"
#include "includes/utils.h"

void shm_ring_init(void *shm_base, size_t shm_region_size, uint64_t channel_key)
{
	shm_ring_t *ring = (shm_ring_t *)shm_base;

	if (shm_region_size < sizeof(shm_ring_t))
	{
		ERROR(
			"%s: Shared memory region size is too small: %zu bytes, "
			"minimum required is %zu bytes\n",
			__func__, shm_region_size, sizeof(shm_ring_t));
		return;
	}

	INFO("%s: Channel publisher_id [%llx]\n", __func__, ring->publisher_id);

	if (ring->key != channel_key)
	{
		ERROR(
			"%s: Channel key mismatch: expected 0x%llx, got 0x%llx\n", __func__,
			channel_key, ring->key);
	}

	// DO NOT NEED TO Initialize the ring buffer structure, hypervisor has done
	// it.
	// memset(shm_base, 0, shm_region_size);

	ring->head = 0;
	ring->tail = 0;
	ring->size = shm_region_size - sizeof(shm_ring_t);
	ring->data_offset = sizeof(shm_ring_t);
}

static inline uint8_t *shm_ring_data_ptr(void *base)
{
	shm_ring_t *ring = (shm_ring_t *)base;
	return (uint8_t *)base + ring->data_offset;
}

// Helper: compute available space
static inline size_t shm_ring_free_space(shm_ring_t *r)
{
	if (r->tail >= r->head)
		return r->size - (r->tail - r->head) - 1;
	else
		return r->head - r->tail - 1;
}

/// @brief Calculate the used space in the shared memory ring buffer.
static inline size_t shm_ring_used_space(shm_ring_t *r)
{
	if (r->tail >= r->head)
		return r->tail - r->head;
	else
		return r->size - (r->head - r->tail);
}

size_t shm_ring_enqueue(void *base, const char __user *data, size_t len)
{
	shm_ring_t *ring = (shm_ring_t *)base;
	uint8_t *buf = shm_ring_data_ptr(base);
	size_t ret = 0;

	size_t tail = ring->tail;

	// Check if the ring buffer has enough space to enqueue the data.
	if (len > ring->size || len > shm_ring_free_space(ring))
	{
		ERROR(
			"%s: Not enough space in the ring buffer to enqueue %zu bytes, "
			"available space: %zu bytes\n",
			__func__, len, shm_ring_free_space(ring));
		return -EAGAIN;
	}

	// Copy data from user space to the ring buffer.
	if (tail + len <= ring->size)
	{
		// write data
		ret = copy_from_user(buf + tail, data, len);
		if (ret < 0)
			goto fail;
		ring->tail = (tail + len) % ring->size;
	}
	else
	{
		// wrap-around: write in two parts
		size_t first_part = ring->size - tail;
		ret = copy_from_user(buf + tail, data, first_part);
		if (ret < 0)
			goto fail;
		// Copy the second part
		ret = copy_from_user(
			buf, (const uint8_t *)data + first_part, len - first_part);
		if (ret < 0)
			goto fail;
		// Update the tail pointer
		ring->tail = (len - first_part);
	}

	ret = len;
	return ret;

fail:
	ERROR(
		"%s: Failed to copy data from user space, error code: %ld\n", __func__,
		ret);
	return ret;
}

/// @brief Dequeue data from the shared memory ring buffer.
/// @param base     Pointer to the base of the shared memory region.
/// @param buf      User-space buffer to store the dequeued data.
/// @param count    Maximum number of bytes to dequeue. If `count` is 0, the
///                 entire slot will be dequeued.
/// @param out_len  Pointer to store the length of the dequeued data.
/// @return `true` if data was dequeued successfully, `false` if the ring is
/// empty.
int shm_ring_dequeue(
	void *base, char __user *buf, size_t count, size_t *out_len)
{
	shm_ring_t *ring = (shm_ring_t *)base;
	uint8_t *data_buf = shm_ring_data_ptr(base);
	size_t len;
	int ret = 0;

	size_t head = ring->head;
	size_t tail = ring->tail;

	size_t used = shm_ring_used_space(ring);

	if (head == tail)
		return false; // empty

	// Calculate how much to read
	len = used;
	if (count > 0 && count < len)
		len = count;

	// Copy data from the ring buffer to the user space buffer
	if (head + len <= ring->size)
	{
		// No wrap-around
		ret = copy_to_user(buf, data_buf + head, len);
		if (ret)
			goto fail;
		ring->head = (head + len) % ring->size;
	}
	else
	{
		// Wrap-around: read in two parts
		size_t first_part = ring->size - head;
		ret = copy_to_user(buf, data_buf + head, first_part);
		if (ret)
			goto fail;
		// Copy the second part
		ret = copy_to_user(buf + first_part, data_buf, len - first_part);
		if (ret)
			goto fail;
		// Update the head pointer
		ring->head = (len - first_part);
	}

	*out_len = len;
	return ret;

fail:
	ERROR(
		"%s: Failed to copy data to user space, error code: %d\n", __func__,
		ret);
	return ret;
}