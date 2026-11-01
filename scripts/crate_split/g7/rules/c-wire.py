RANK={'medulla-wire-core':1,'medulla-wire':2,'medulla-wire-msg':2}
RULES=[(x,'medulla-wire-core') for x in ['wire','broker_wire','sql_wire_auth']]+[(x,'medulla-wire-msg') for x in ['amqp_wire','mqtt_wire','stomp_wire','dds','ros2_bridge','redis_wire','s3','kvcache_http']]
DEFAULT='medulla-wire'
