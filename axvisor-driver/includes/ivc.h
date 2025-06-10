#ifndef __IVC_H__
#define __IVC_H__

#define IVC_PUBLISHER_DEV_NAME "axivc_publisher"
#define IVC_SUBSCRIBER_DEV_NAME "axivc_subscriber"

int init_ivc_devices(void);
void uninit_ivc_devices(void);

struct ivc_shm_header
{
	u64 publisher_id;
	u64 key;
	u64 content_size;
};

struct ivc_subscribe_arg
{
	u64 target_publisher_id;
	u64 channel_key;
};

#define IVC_SUBSCRIBE_CHANNEL _IOW(0, 0, struct ivc_subscribe_arg)
#define IVC_UNSUBSCRIBE_CHANNEL _IOW(0, 1, struct ivc_subscribe_arg)

#endif // __IVC_H__