package main

import (
	"fmt"
	"time"

	"github.com/blanklabel/meldworld/model"
	"github.com/satori/go.uuid" // TODO REMOVE WHEN RECORD PULLED
	"github.com/sirupsen/logrus"
	log "github.com/sirupsen/logrus"
)

type GameHub struct {
	clients      map[string]*model.Player
	broadcast    chan *model.ClientMessage
	register     chan *model.Player
	unregister   chan model.Player
	entityaction chan *model.EntityAction
	actionqueue  *LIFOQueue
	WorldMapped  model.WorldMap
}

// AddNewClient Add a new unknown client
func (g *GameHub) AddNewClient(p *model.Player) {
	// map the player to the hub
	g.clients[p.ID] = p

	// Notify everyone of the new player
	announcement := fmt.Sprintf("Player %s joined", p.ID)
	r := &model.ClientMessage{ModelType: model.ModelType{MsgType: model.CLIENTJOIN},
		Msg: announcement, Sender: "Server"}
	g.Broadcast(r)

	// TODO: Pull new record
	gh.WorldMapped.Entities = append(gh.WorldMapped.Entities,
		model.Entity{
			ID:          uuid.NewV4().String(),
			OwnerID:     p.ID,
			Name:        "Bob",
			Full_hp:     20,
			C_hp:        20,
			Phy_def:     3,
			Phy_atk:     2,
			Speed:       1,
			Coordinates: model.Cords{X: 0, Y: 0},
			Destination: model.Cords{X: 0, Y: 0},
		})

	//// TODO: Entities as maps
	//entID := uuid.NewV4().String()
	//gh.WorldMapped.Entities[entID] =
	//    model.Entity{
	//        ID:          entID,
	//        OwnerID:     p.ID,
	//        Name:        "Bob",
	//        Full_hp:     20,
	//        C_hp:        20,
	//        Phy_def:     3,
	//        Phy_atk:     2,
	//        Speed:       1,
	//        Coordinates: model.Cords{X: 0, Y: 0},
	//        Destination: model.Cords{X: 0, Y: 0},
	//    }

	// Tell the player who they are
	g.DirectMessage(p.PlayerInfo, p.ID)

	// Give the new player the worldmap
	g.DirectMessage(g.WorldMapped, p.ID)

}

// RemoveClient Remove a client from the server
func (g *GameHub) RemoveClient(p model.Player) {
	// close the connection and delete the record
	g.clients[p.ID].WSconn.Close()
	delete(g.clients, p.ID)

	// Notify server of player departure
	announcement := fmt.Sprintf("Player %s left", p.ID)
	r := &model.ClientMessage{ModelType: model.ModelType{MsgType: model.CLIENTLEAVE},
		Msg: announcement, Sender: "Server"}
	g.Broadcast(r)
}

// DirectMessage Write a message to a single client
func (g GameHub) DirectMessage(msg interface{}, userID string) {
	// TODO: Should track FROM & TO maybe?

	err := g.clients[userID].WSconn.WriteJSON(msg)
	if err != nil {
		log.Warn("Message Failed to send:", err, msg)
	}
}

// Broadcast speak to all clients connected
func (g GameHub) Broadcast(msg *model.ClientMessage) {

	log.WithFields(logrus.Fields{
		"messagetype": msg.MsgType,
		"content":     msg.Msg,
	}).Info("Message sent")

	// loop through all clients and give them a message
	for id, client := range g.clients {
		err := client.WSconn.WriteJSON(msg)
		if err != nil {
			log.Warn("Didn't send due to error:", err, id)
		}
	}
}

// UpdateEntityBroadcast notify all clients of an entity action
func (g GameHub) UpdateEntityBroadcast(entity *model.Entity) {
	entity.ModelType.MsgType = model.ENTITY

	// loop through all clients and give them a message
	for _, client := range g.clients {
		err := client.WSconn.WriteJSON(entity)
		if err != nil {
			log.Warn("Didn't send due to error:", err)
		}
	}
}

func (g GameHub) isMoveValid(move *model.EntityMove) {
	// something something moves valid?
}

func (g GameHub) EntityAction(action *model.EntityAction) {
	switch action.Action {
	case model.ENTITYACTIONMOVE:
		log.WithFields(logrus.Fields{
			"ownerID":    action.OwnerID,
			"entityID":   action.ID,
			"actiontype": action.Action,
			"direction":  action.Direction,
			"distance":   action.Distance,
		}).Info("ACTION")

		// TODO: if the move command is valid
		gh.actionqueue.Push(action)

	default:
		log.Warn("Unknown Action")
	}
}

func (g GameHub) ServeGame() {

	//running at 60 FPS everyone get the ticks  -- int(1e9) / 60
	// Once a second everyone gets an update
	frameNS := time.Duration(time.Second)
	clk := time.NewTicker(frameNS)

	// Run forever
	for {
		select {
		// main loop
		case <-clk.C:
			// loop through the queue
			i := 0
			for i < gh.actionqueue.GetSize() {
				action := gh.actionqueue.Pop()
				log.Debug("Recieved Action", action.ID, action.OwnerID)
				// loop through all entities
				// TODO change entities to a map
				for index, entity := range gh.WorldMapped.Entities {
					if entity.ID == action.Entity.ID {
						switch action.Direction {
						case model.ENTITYDIRECTIONUP:
							newCord := gh.WorldMapped.Entities[index].Coordinates.Y - action.Distance
							if newCord >= 0 {
								gh.WorldMapped.Entities[index].Coordinates.Y = newCord
							}

						case model.ENTITYDIRECTIONDOWN:
							newCord := gh.WorldMapped.Entities[index].Coordinates.Y + action.Distance
							if newCord < gh.WorldMapped.Dimensions.Height {
								gh.WorldMapped.Entities[index].Coordinates.Y = newCord
							}

						case model.ENTITYDIRECTIONLEFT:
							newCord := gh.WorldMapped.Entities[index].Coordinates.X - action.Distance
							if newCord >= 0 {
								gh.WorldMapped.Entities[index].Coordinates.X = newCord
							}

						case model.ENTITYDIRECTIONRIGHT:
							newCord := gh.WorldMapped.Entities[index].Coordinates.X + action.Distance
							if newCord < gh.WorldMapped.Dimensions.Width {
								gh.WorldMapped.Entities[index].Coordinates.X = newCord
							}
						}
						e := &model.Entity{
							OwnerID: entity.OwnerID,
							ID:      entity.ID,
							Coordinates: model.Cords{
								X: entity.Coordinates.X,
								Y: entity.Coordinates.Y,
							},
						}

						log.Debug("UPDATING ENTITY", e)
						gh.UpdateEntityBroadcast(e)
					}
				}
			}
			// TODO: Update world map with new activity
			// Go through a queue of actions - Done
			// if location isn't a collision announce destination
			// if is valid (Speed + tile direction) allow move
			// else return error to client

			// announce new locations
		case c := <-gh.register:
			gh.AddNewClient(c)
		case c := <-gh.unregister:
			gh.RemoveClient(c)
		case c := <-gh.broadcast:
			gh.Broadcast(c)
		case c := <-gh.entityaction:
			gh.EntityAction(c)
		}
	}
}
